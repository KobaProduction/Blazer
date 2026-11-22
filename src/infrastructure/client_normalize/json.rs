use std::{cell::Cell, fmt, net::IpAddr};

use serde::de::{self, Deserialize, DeserializeSeed, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value};
use vpn_application::client::{ProfileUnavailableReason, UnavailableProfile};
use vpn_domain::client::TcpProbeEndpoint;

use super::subscription::{SubscriptionProfiles, is_tcp_transport, protocol_label};
use super::{
    AlpnProtocols, ClientError, MAX_PROFILES, ProxyProfile, blazecore, endpoint_host, links,
    sing_box, xray, xray_graph,
};

pub(super) fn parse_document(text: &str) -> Result<Vec<ProxyProfile>, ClientError> {
    parse_document_value(parse_value(text)?)
}

pub(super) fn recognizes_native_yaml(text: &str) -> bool {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('#') && *line != "---")
        .and_then(|line| line.strip_prefix("- ").unwrap_or(line).split_once(':'))
        .is_some_and(|(key, _)| {
            matches!(
                key,
                "address" | "name" | "protocol" | "client_group" | "transport"
            )
        })
}

pub(super) fn parse_native_yaml(text: &str) -> Result<Vec<ProxyProfile>, ClientError> {
    parse_document_value(parse_yaml_value(text)?)
}

fn parse_document_value(value: Value) -> Result<Vec<ProxyProfile>, ClientError> {
    if !value.is_array() && !value.is_object() {
        return Err(ClientError::UnsupportedFormat);
    }
    let items = value
        .as_array()
        .map_or(std::slice::from_ref(&value), Vec::as_slice);
    let groups = blazecore::expand_client_groups(items)?;
    let items = groups.as_deref().unwrap_or(items);
    let mut profiles = Vec::new();
    for item in items {
        append_profiles(item, &mut profiles)?;
    }
    if profiles.is_empty() {
        return Err(ClientError::InvalidSubscription);
    }
    Ok(profiles)
}

fn append_profiles(value: &Value, profiles: &mut Vec<ProxyProfile>) -> Result<(), ClientError> {
    if let Some(outbounds) = value.get("outbounds") {
        return append_wrapped_outbounds(value, outbounds, profiles);
    }
    if profiles.len() == MAX_PROFILES {
        return Err(ClientError::PayloadTooLarge);
    }
    profiles.push(match value.as_str() {
        Some(link) => links::parse_share_link(link.trim())?,
        None => parse_outbound(value)?,
    });
    Ok(())
}

fn append_wrapped_outbounds(
    document: &Value,
    outbounds: &Value,
    profiles: &mut Vec<ProxyProfile>,
) -> Result<(), ClientError> {
    let outbounds = outbounds
        .as_array()
        .ok_or(ClientError::InvalidSubscription)?;
    if let Some(outbound) = standalone_xray_xhttp(document, outbounds) {
        if profiles.len() == MAX_PROFILES {
            return Err(ClientError::PayloadTooLarge);
        }
        profiles.push(parse_wrapped_xhttp(document, outbound)?);
        return Ok(());
    }
    if xray_graph::recognizes(document) {
        if profiles.len() == MAX_PROFILES {
            return Err(ClientError::PayloadTooLarge);
        }
        profiles.push(xray_graph::parse(document)?);
        return Ok(());
    }
    fields(document, &["outbounds", "inbounds", "log"])?;
    if outbounds.is_empty() {
        return Err(ClientError::InvalidSubscription);
    }
    let xray = outbounds
        .iter()
        .all(|outbound| outbound.get("protocol").is_some_and(Value::is_string));
    wrapper_options(document, xray)?;
    if outbounds.len() > MAX_PROFILES - profiles.len() {
        return Err(ClientError::PayloadTooLarge);
    }
    for outbound in outbounds {
        if static_xray_outbound(outbound, xray)? {
            continue;
        }
        profiles.push(parse_outbound(outbound)?);
    }
    Ok(())
}

fn wrapper_options(document: &Value, xray: bool) -> Result<(), ClientError> {
    check_empty(document, "inbounds", |value| {
        value
            .as_array()
            .is_some_and(|items| xray || items.is_empty())
    })?;
    check_empty(document, "log", |value| {
        value
            .as_object()
            .is_some_and(|items| xray || items.is_empty())
    })
}

fn static_xray_outbound(outbound: &Value, xray: bool) -> Result<bool, ClientError> {
    let protocol = outbound.get("protocol").and_then(Value::as_str);
    let static_outbound = xray && matches!(protocol, Some("freedom" | "blackhole"));
    if static_outbound {
        fields(outbound, &["protocol", "tag", "settings"])?;
    }
    Ok(static_outbound)
}

fn parse_outbound(value: &Value) -> Result<ProxyProfile, ClientError> {
    let mut profile = if value.get("containers").is_some() {
        super::amnezia::parse_bundle(value)
    } else {
        match (value.get("protocol"), value.get("type")) {
            (Some(protocol), None) if protocol.is_string() => xray::parse_outbound(value),
            (Some(protocol), None) if protocol.is_object() => blazecore::parse(value),
            (None, Some(_)) => sing_box::parse_outbound(value),
            (Some(_), Some(_)) => Err(ClientError::UnsupportedFeature),
            (_, None) => Err(ClientError::UnsupportedFormat),
        }
    }?;
    let source =
        serde_json::to_string_pretty(value).map_err(|_| ClientError::InvalidSubscription)?;
    profile.source = Some(vpn_domain::client::Secret::new(source));
    Ok(profile)
}

pub(super) fn append_subscription_entries(
    text: &str,
    profiles: &mut SubscriptionProfiles,
) -> Result<(), ClientError> {
    let value = if recognizes_native_yaml(text) {
        parse_yaml_value(text)?
    } else {
        parse_value(text)?
    };
    let items = value
        .as_array()
        .map_or(std::slice::from_ref(&value), Vec::as_slice);
    let groups = blazecore::expand_client_groups(items)?;
    let items = groups.as_deref().unwrap_or(items);
    for item in items {
        if ["error", "errors", "message"]
            .iter()
            .any(|key| item.get(key).is_some())
        {
            return Err(ClientError::InvalidSubscription);
        }
        match (item.as_str(), item.get("outbounds")) {
            (Some(link), _) => profiles.append_link(link.trim())?,
            (_, Some(outbounds)) => append_subscription_wrapper(item, outbounds, profiles)?,
            _ => append_subscription_outbound(item, None, profiles)?,
        }
    }
    Ok(())
}

fn append_subscription_wrapper(
    document: &Value,
    outbounds: &Value,
    profiles: &mut SubscriptionProfiles,
) -> Result<(), ClientError> {
    let Some(outbounds) = outbounds.as_array() else {
        return append_subscription_outbound(document, None, profiles);
    };
    if outbounds.len() > MAX_PROFILES {
        return Err(ClientError::PayloadTooLarge);
    }
    if !outbounds.iter().any(is_proxy_outbound) {
        return append_subscription_outbound(document, None, profiles);
    }
    let name = optional_str(document, "remarks")?.unwrap_or_default();
    if let Some(outbound) = standalone_xray_xhttp(document, outbounds) {
        return append_normalized_outbound(
            outbound,
            Some(name),
            parse_wrapped_xhttp(document, outbound),
            profiles,
        );
    }
    if xray_graph::recognizes(document) {
        return profiles.append(xray_graph::parse(document), true, |reason| {
            unavailable_wrapper(outbounds, name, reason)
        });
    }
    let xray = outbounds
        .iter()
        .filter(|outbound| is_proxy_outbound(outbound))
        .all(|outbound| outbound.get("protocol").is_some_and(Value::is_string));
    if !independent_outbounds(document, outbounds, xray)? {
        return append_unavailable_wrapper(outbounds, name, profiles);
    }
    let single_proxy = outbounds
        .iter()
        .filter(|value| is_proxy_outbound(value))
        .count()
        == 1;
    for outbound in outbounds {
        if !static_xray_outbound(outbound, true)? {
            append_subscription_outbound(outbound, single_proxy.then_some(name), profiles)?;
        }
    }
    Ok(())
}

fn append_unavailable_wrapper(
    outbounds: &[Value],
    name: &str,
    profiles: &mut SubscriptionProfiles,
) -> Result<(), ClientError> {
    profiles.append(Err(ClientError::UnsupportedFeature), true, |reason| {
        unavailable_wrapper(outbounds, name, reason)
    })
}

fn unavailable_wrapper(
    outbounds: &[Value],
    name: &str,
    reason: ProfileUnavailableReason,
) -> UnavailableProfile {
    let protocol = wrapper_protocol(outbounds);
    let mut proxies = outbounds
        .iter()
        .filter(|outbound| is_proxy_outbound(outbound));
    let only_proxy = proxies.next().filter(|_| proxies.next().is_none());
    UnavailableProfile::new(name, protocol, reason)
        .with_probe_endpoint(only_proxy.and_then(tcp_probe_endpoint))
}

fn standalone_xray_xhttp<'a>(document: &Value, outbounds: &'a [Value]) -> Option<&'a Value> {
    let (proxy, static_outbounds) = outbounds.split_first()?;
    // AsIs and no rules leave the first outbound as the only route. A plain global DNS
    // list is unused by this route; its outbound has no local DNS resolution directive.
    (is_proxy_outbound(proxy)
        && proxy.get("protocol").is_some_and(Value::is_string)
        && xray::uses_xhttp(proxy)
        && fields(
            document,
            &["outbounds", "inbounds", "log", "remarks", "routing", "dns"],
        )
        .is_ok()
        && wrapper_options(document, true).is_ok()
        && check_empty(document, "inbounds", |value| {
            value.as_array().is_some_and(|inbounds| {
                inbounds.iter().all(|inbound| {
                    matches!(
                        inbound.get("protocol").and_then(Value::as_str),
                        Some("socks" | "http")
                    )
                })
            })
        })
        .is_ok()
        && baseline_xray_routing(document)
        && static_outbounds.iter().all(|outbound| {
            matches!(static_xray_outbound(outbound, true), Ok(true))
                && check_empty(outbound, "settings", |settings| {
                    settings.as_object().is_some_and(Map::is_empty)
                })
                .is_ok()
        }))
    .then_some(proxy)
}

fn baseline_xray_routing(document: &Value) -> bool {
    check_empty(document, "routing", |routing| {
        fields(routing, &["domainStrategy", "rules"]).is_ok()
            && check_empty(routing, "domainStrategy", |strategy| {
                strategy.as_str() == Some("AsIs")
            })
            .is_ok()
            && check_empty(routing, "rules", |rules| {
                rules.as_array().is_some_and(Vec::is_empty)
            })
            .is_ok()
    })
    .is_ok()
        && check_empty(document, "dns", |dns| {
            fields(dns, &["servers"]).is_ok()
                && check_empty(dns, "servers", |servers| {
                    servers.as_array().is_some_and(|servers| {
                        servers.iter().all(|server| {
                            server
                                .as_str()
                                .is_some_and(|ip| ip.parse::<IpAddr>().is_ok())
                        })
                    })
                })
                .is_ok()
        })
        .is_ok()
}

fn parse_wrapped_xhttp(document: &Value, outbound: &Value) -> Result<ProxyProfile, ClientError> {
    let mut profile = parse_outbound(outbound)?;
    apply_wrapper_name(&mut profile, optional_str(document, "remarks")?);
    let source =
        serde_json::to_string_pretty(document).map_err(|_| ClientError::InvalidSubscription)?;
    profile.source = Some(vpn_domain::client::Secret::new(source));
    Ok(profile)
}

fn independent_outbounds(
    document: &Value,
    outbounds: &[Value],
    xray: bool,
) -> Result<bool, ClientError> {
    // Only recognize configuration sections here. An API error envelope may also have an outbounds field.
    fields(
        document,
        &[
            "outbounds",
            "inbounds",
            "log",
            "remarks",
            "routing",
            "route",
            "dns",
            "api",
            "stats",
            "policy",
            "observatory",
            "burstObservatory",
            "fakedns",
            "reverse",
            "services",
            "endpoints",
            "experimental",
            "ntp",
            "metrics",
            "transport",
        ],
    )
    .map_err(|_| ClientError::InvalidSubscription)?;
    Ok(
        fields(document, &["outbounds", "inbounds", "log", "remarks"]).is_ok()
            && wrapper_options(document, xray).is_ok()
            && outbounds
                .iter()
                .all(|outbound| static_xray_outbound(outbound, true).is_ok()),
    )
}

fn wrapper_protocol(outbounds: &[Value]) -> Option<&'static str> {
    let mut protocols = outbounds
        .iter()
        .filter(|value| is_proxy_outbound(value))
        .map(outbound_protocol);
    let first = protocols.next().flatten()?;
    protocols.all(|label| label == Some(first)).then_some(first)
}

fn append_subscription_outbound(
    outbound: &Value,
    wrapper_name: Option<&str>,
    profiles: &mut SubscriptionProfiles,
) -> Result<(), ClientError> {
    if !is_proxy_outbound(outbound) {
        return profiles.append(Err(ClientError::InvalidSubscription), false, |reason| {
            UnavailableProfile::new("", None, reason)
        });
    }
    let mut result = parse_outbound(outbound);
    if let Ok(profile) = &mut result {
        apply_wrapper_name(profile, wrapper_name);
    }
    append_normalized_outbound(outbound, wrapper_name, result, profiles)
}

fn apply_wrapper_name(profile: &mut ProxyProfile, name: Option<&str>) {
    if let Some(name) = name {
        let label = UnavailableProfile::new(name, None, ProfileUnavailableReason::Unsupported).name;
        if !label.is_empty() {
            profile.name = label;
        }
    }
}

fn append_normalized_outbound(
    outbound: &Value,
    wrapper_name: Option<&str>,
    result: Result<ProxyProfile, ClientError>,
    profiles: &mut SubscriptionProfiles,
) -> Result<(), ClientError> {
    profiles.append(result, true, |reason| {
        let name = wrapper_name
            .filter(|name| !name.is_empty())
            .or_else(|| outbound.get("tag").and_then(Value::as_str));
        let row = UnavailableProfile::new(
            name.unwrap_or_default(),
            outbound_protocol(outbound),
            reason,
        );
        if reason.supports_tcp_probe() {
            return row.with_probe_endpoint(tcp_probe_endpoint(outbound));
        }
        row
    })
}

fn tcp_probe_endpoint(outbound: &Value) -> Option<TcpProbeEndpoint> {
    if ["detour", "proxySettings", "sendThrough"]
        .iter()
        .any(|key| outbound.get(key).is_some())
    {
        return None;
    }
    let (host, port) = if let Some(kind) = outbound.get("protocol").and_then(Value::as_str) {
        let stream = outbound.get("streamSettings");
        if let Some(stream) = stream {
            if !stream.is_object() || stream.get("sockopt").is_some() {
                return None;
            }
            if stream
                .get("network")
                .is_some_and(|network| !network.as_str().is_some_and(is_tcp_transport))
            {
                return None;
            }
        }
        let server = xray::endpoint(kind, outbound.get("settings")?).ok()?;
        (string(server, "address").ok()?, port(server, "port").ok()?)
    } else {
        sing_box_tcp_address(outbound)?
    };
    TcpProbeEndpoint::new(&endpoint_host(host).ok()?, port)
}

fn sing_box_tcp_address(outbound: &Value) -> Option<(&str, u16)> {
    let kind = outbound.get("type")?.as_str()?;
    if !matches!(
        kind,
        "socks" | "http" | "vless" | "vmess" | "trojan" | "shadowsocks"
    ) || outbound
        .get("network")
        .is_some_and(|network| !matches!(network.as_str(), Some("" | "tcp")))
        || outbound.get("transport").is_some_and(|transport| {
            !transport
                .get("type")
                .and_then(Value::as_str)
                .is_some_and(is_tcp_transport)
        })
    {
        return None;
    }
    Some((
        string(outbound, "server").ok()?,
        port(outbound, "server_port").ok()?,
    ))
}

fn is_proxy_outbound(value: &Value) -> bool {
    if value.get("containers").is_some() {
        return true;
    }
    if static_xray_outbound(value, true).unwrap_or(true) {
        return false;
    }
    outbound_protocol(value).is_some()
        || value.get("protocol").is_some_and(|protocol| {
            protocol.is_string() && value.get("settings").is_some()
                || protocol.is_object() && value.get("address").is_some()
        })
        || value.get("type").is_some_and(Value::is_string)
            && (value.get("server").is_some() || value.get("server_port").is_some())
}

fn outbound_protocol(value: &Value) -> Option<&'static str> {
    if value.get("containers").is_some() {
        return Some("amnezia");
    }
    value
        .get("protocol")
        .and_then(|value| {
            value
                .as_str()
                .or_else(|| value.get("type").and_then(Value::as_str))
        })
        .or_else(|| value.get("type").and_then(Value::as_str))
        .and_then(protocol_label)
}

pub(crate) fn parse_value(input: &str) -> Result<Value, ClientError> {
    serde_json::from_str::<UniqueValue>(input)
        .map(|value| value.0)
        .map_err(|_| ClientError::InvalidSubscription)
}

fn parse_yaml_value(input: &str) -> Result<Value, ClientError> {
    serde_yaml::from_str::<UniqueValue>(input)
        .map(|value| value.0)
        .map_err(|_| ClientError::InvalidSubscription)
}

pub(super) fn fields(value: &Value, allowed: &[&str]) -> Result<(), ClientError> {
    let object = value.as_object().ok_or(ClientError::InvalidSubscription)?;
    if object.keys().any(|key| !allowed.contains(&key.as_str())) {
        return Err(ClientError::UnsupportedFeature);
    }
    Ok(())
}

pub(super) fn string<'a>(value: &'a Value, key: &str) -> Result<&'a str, ClientError> {
    optional_str(value, key)?.ok_or(ClientError::InvalidSubscription)
}

pub(super) fn optional_str<'a>(
    value: &'a Value,
    key: &str,
) -> Result<Option<&'a str>, ClientError> {
    value
        .get(key)
        .map(|field| field.as_str().ok_or(ClientError::InvalidSubscription))
        .transpose()
}

pub(super) fn flag(value: &Value, key: &str) -> Result<bool, ClientError> {
    value
        .get(key)
        .map(|field| field.as_bool().ok_or(ClientError::InvalidSubscription))
        .transpose()
        .map(Option::unwrap_or_default)
}

pub(super) fn tls_alpn(value: &Value) -> Result<AlpnProtocols, ClientError> {
    let Some(field) = value.get("alpn") else {
        return Ok(AlpnProtocols::default());
    };
    let protocols = field.as_array().ok_or(ClientError::InvalidSubscription)?;
    let protocols = protocols
        .iter()
        .map(|protocol| {
            protocol
                .as_str()
                .map(str::to_owned)
                .ok_or(ClientError::InvalidSubscription)
        })
        .collect::<Result<Vec<_>, _>>()?;
    AlpnProtocols::new(protocols).map_err(|_| ClientError::InvalidSubscription)
}

pub(super) fn port(value: &Value, key: &str) -> Result<u16, ClientError> {
    let field = value.get(key).ok_or(ClientError::InvalidSubscription)?;
    field
        .as_u64()
        .and_then(|value| u16::try_from(value).ok())
        .or_else(|| field.as_str().and_then(|value| value.parse().ok()))
        .filter(|value| *value != 0)
        .ok_or(ClientError::InvalidSubscription)
}

pub(super) fn check_empty(
    value: &Value,
    key: &str,
    is_default: impl FnOnce(&Value) -> bool,
) -> Result<(), ClientError> {
    if value.get(key).is_some_and(|option| !is_default(option)) {
        return Err(ClientError::UnsupportedFeature);
    }
    Ok(())
}

pub(super) fn zero(value: &Value) -> bool {
    value.as_u64() == Some(0) || value.as_str() == Some("0")
}

pub(super) fn host_header(value: &Value) -> Result<Option<&str>, ClientError> {
    let Some(headers) = value.get("headers") else {
        return Ok(None);
    };
    let headers = headers
        .as_object()
        .ok_or(ClientError::InvalidSubscription)?;
    match headers.iter().next() {
        None => Ok(None),
        Some((key, value)) if key.eq_ignore_ascii_case("host") && headers.len() == 1 => value
            .as_str()
            .map(Some)
            .ok_or(ClientError::InvalidSubscription),
        _ => Err(ClientError::UnsupportedFeature),
    }
}

struct UniqueValue(Value);

impl<'de> Deserialize<'de> for UniqueValue {
    fn deserialize<D: Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
        // Bound the expanded representation, including data reached through YAML aliases.
        let remaining = Cell::new(16 * vpn_application::MAX_SUBSCRIPTION_BYTES);
        UniqueVisitor(&remaining).deserialize(decoder).map(Self)
    }
}

#[derive(Clone, Copy)]
struct UniqueVisitor<'a>(&'a Cell<usize>);

impl UniqueVisitor<'_> {
    fn charge<E: de::Error>(&self, bytes: usize) -> Result<(), E> {
        let left = self
            .0
            .get()
            .checked_sub(bytes)
            .ok_or_else(|| E::custom("expanded configuration exceeds size limit"))?;
        self.0.set(left);
        Ok(())
    }
}

impl<'de> DeserializeSeed<'de> for UniqueVisitor<'_> {
    type Value = Value;

    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<Value, D::Error> {
        self.charge(size_of::<Value>())?;
        decoder.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for UniqueVisitor<'_> {
    type Value = Value;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a configuration value with unique keys")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut access: A) -> Result<Value, A::Error> {
        let mut object = Map::new();
        while let Some(key) = access.next_key::<String>()? {
            self.charge(key.len() + size_of::<String>())?;
            let value = access.next_value_seed(self)?;
            if object.insert(key, value).is_some() {
                return Err(de::Error::custom("duplicate configuration key"));
            }
        }
        Ok(Value::Object(object))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut access: A) -> Result<Value, A::Error> {
        let mut values = Vec::new();
        while let Some(value) = access.next_element_seed(self)? {
            values.push(value);
        }
        Ok(Value::Array(values))
    }

    fn visit_bool<E: de::Error>(self, value: bool) -> Result<Value, E> {
        Ok(Value::Bool(value))
    }

    fn visit_i64<E: de::Error>(self, value: i64) -> Result<Value, E> {
        Ok(Value::Number(Number::from(value)))
    }

    fn visit_u64<E: de::Error>(self, value: u64) -> Result<Value, E> {
        Ok(Value::Number(Number::from(value)))
    }

    fn visit_f64<E: de::Error>(self, value: f64) -> Result<Value, E> {
        Number::from_f64(value)
            .map(Value::Number)
            .ok_or_else(|| E::custom("non-finite number"))
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<Value, E> {
        self.charge(value.len())?;
        Ok(Value::String(value.to_owned()))
    }

    fn visit_string<E: de::Error>(self, value: String) -> Result<Value, E> {
        self.charge(value.len())?;
        Ok(Value::String(value))
    }

    fn visit_unit<E: de::Error>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }
}
