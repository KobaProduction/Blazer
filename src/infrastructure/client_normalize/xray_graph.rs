//! Strict conversion of one complete Xray routing graph into client contracts.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::time::Duration;

use serde_json::Value;
use vpn_domain::client::{
    GraphGroup, GraphMatch, GraphNetwork, GraphObservatory, GraphOutbound, GraphOutboundKind,
    GraphRule, GraphStrategy, GraphTarget, ProxyGraph,
};
use vpn_domain::routing::DnsPolicy;

use super::json::{check_empty, fields, optional_str, port, string, zero};
use super::{
    ClientError, MAX_PROFILES, ProxyProfile, ProxyProtocol, ProxySecurity, ProxyTransport, Secret,
    endpoint_host, make_profile, xray,
};

pub(super) fn recognizes(document: &Value) -> bool {
    document.get("routing").is_some()
        && document
            .get("outbounds")
            .and_then(Value::as_array)
            .is_some_and(|outbounds| {
                !outbounds.is_empty()
                    && outbounds
                        .iter()
                        .all(|outbound| outbound.get("protocol").is_some_and(Value::is_string))
            })
}

pub(super) fn parse(document: &Value) -> Result<ProxyProfile, ClientError> {
    fields(
        document,
        &[
            "remarks",
            "outbounds",
            "inbounds",
            "log",
            "routing",
            "dns",
            "observatory",
        ],
    )?;
    validate_log(document)?;
    validate_dns(document)?;
    let inbound = validate_inbound(document)?;
    let outbounds = array(document, "outbounds")?
        .iter()
        .map(parse_outbound)
        .collect::<Result<Vec<_>, _>>()?;
    let routing = document
        .get("routing")
        .ok_or(ClientError::InvalidSubscription)?;
    fields(routing, &["domainStrategy", "rules", "balancers"])?;
    check_empty(routing, "domainStrategy", |value| {
        value.as_str() == Some("AsIs")
    })?;
    let groups = parse_groups(routing, &outbounds)?;
    let rules = array(routing, "rules")?
        .iter()
        .map(|rule| parse_rule(rule, inbound.as_deref()))
        .collect::<Result<Vec<_>, _>>()?;
    let first = outbounds.first().ok_or(ClientError::InvalidSubscription)?;
    let default_target = GraphTarget::Outbound(first.tag.clone());
    let observatory = parse_observatory(document, &outbounds)?;
    let graph = ProxyGraph {
        outbounds,
        groups,
        rules,
        default_target,
        dns: DnsPolicy::Automatic,
        observatory,
    };
    graph.validate().map_err(graph_error)?;
    graph_profile(document, graph)
}

fn graph_error(error: ClientError) -> ClientError {
    match error {
        ClientError::InvalidInput | ClientError::InvalidUrl => ClientError::InvalidSubscription,
        error => error,
    }
}

fn array<'a>(document: &'a Value, key: &str) -> Result<&'a [Value], ClientError> {
    let Some(value) = document.get(key) else {
        return Ok(&[]);
    };
    let values = value.as_array().ok_or(ClientError::InvalidSubscription)?;
    if values.len() > MAX_PROFILES {
        return Err(ClientError::PayloadTooLarge);
    }
    Ok(values)
}

fn validate_log(document: &Value) -> Result<(), ClientError> {
    let Some(log) = document.get("log") else {
        return Ok(());
    };
    fields(log, &["loglevel"])?;
    check_empty(log, "loglevel", |level| {
        matches!(
            level.as_str(),
            Some("debug" | "info" | "warning" | "error" | "none")
        )
    })
}

fn validate_dns(document: &Value) -> Result<(), ClientError> {
    let Some(dns) = document.get("dns") else {
        return Ok(());
    };
    fields(dns, &["servers"])?;
    check_empty(dns, "servers", |servers| {
        servers.as_array().is_some_and(Vec::is_empty)
    })
}

fn validate_inbound(document: &Value) -> Result<Option<String>, ClientError> {
    let inbounds = array(document, "inbounds")?;
    let inbound = match inbounds {
        [] => return Ok(None),
        [inbound] => inbound,
        _ => return Err(ClientError::UnsupportedFeature),
    };
    fields(
        inbound,
        &[
            "tag",
            "listen",
            "port",
            "protocol",
            "settings",
            "sniffing",
            "streamSettings",
        ],
    )?;
    if !matches!(string(inbound, "protocol")?, "socks" | "http") {
        return Err(ClientError::UnsupportedFeature);
    }
    check_empty(inbound, "listen", |value| {
        matches!(value.as_str(), Some("127.0.0.1" | "::1" | "localhost"))
    })?;
    if inbound.get("port").is_some() {
        port(inbound, "port")?;
    }
    validate_inbound_settings(inbound)?;
    if let Some(sniffing) = inbound.get("sniffing") {
        fields(sniffing, &["enabled"])?;
        check_empty(sniffing, "enabled", |value| value.as_bool() == Some(false))?;
    }
    if let Some(stream) = inbound.get("streamSettings") {
        fields(stream, &["network", "security"])?;
        check_empty(stream, "network", |value| value.as_str() == Some("tcp"))?;
        check_empty(stream, "security", |value| value.as_str() == Some("none"))?;
    }
    Ok(optional_str(inbound, "tag")?.map(str::to_owned))
}

fn validate_inbound_settings(inbound: &Value) -> Result<(), ClientError> {
    let Some(settings) = inbound.get("settings") else {
        return Ok(());
    };
    let allowed: &[&str] = if string(inbound, "protocol")? == "socks" {
        &["auth", "accounts", "udp", "userLevel"]
    } else {
        &["accounts", "allowTransparent", "userLevel"]
    };
    fields(settings, allowed)?;
    check_empty(settings, "auth", |auth| auth.as_str() == Some("noauth"))?;
    check_empty(settings, "accounts", |accounts| {
        accounts.as_array().is_some_and(Vec::is_empty)
    })?;
    check_empty(settings, "udp", Value::is_boolean)?;
    check_empty(settings, "allowTransparent", |value| {
        value.as_bool() == Some(false)
    })?;
    check_empty(settings, "userLevel", zero)
}

fn parse_outbound(outbound: &Value) -> Result<GraphOutbound, ClientError> {
    let tag = string(outbound, "tag")?.to_owned();
    let kind = match string(outbound, "protocol")? {
        "freedom" => {
            validate_static(outbound)?;
            GraphOutboundKind::Direct
        }
        "blackhole" => {
            validate_static(outbound)?;
            GraphOutboundKind::Block
        }
        _ => {
            let mut profile = xray::parse_outbound(outbound)?;
            profile.source = Some(Secret::new(
                serde_json::to_string_pretty(outbound)
                    .map_err(|_| ClientError::InvalidSubscription)?,
            ));
            GraphOutboundKind::Proxy(Box::new(profile))
        }
    };
    Ok(GraphOutbound { tag, kind })
}

fn validate_static(outbound: &Value) -> Result<(), ClientError> {
    fields(outbound, &["tag", "protocol", "settings"])?;
    check_empty(outbound, "settings", |settings| {
        settings.as_object().is_some_and(serde_json::Map::is_empty)
    })
}

fn parse_groups(
    routing: &Value,
    outbounds: &[GraphOutbound],
) -> Result<Vec<GraphGroup>, ClientError> {
    array(routing, "balancers")?
        .iter()
        .map(|group| {
            fields(group, &["tag", "selector", "strategy", "fallbackTag"])?;
            let strategy = parse_strategy(group)?;
            let selectors = strings(group, "selector")?;
            if selectors.is_empty() {
                return Err(ClientError::InvalidSubscription);
            }
            let members = outbounds
                .iter()
                .filter(|outbound| {
                    selectors
                        .iter()
                        .any(|prefix| outbound.tag.starts_with(prefix))
                })
                .map(|outbound| outbound.tag.clone())
                .collect();
            Ok(GraphGroup {
                tag: string(group, "tag")?.to_owned(),
                members,
                strategy,
            })
        })
        .collect()
}

fn parse_strategy(group: &Value) -> Result<GraphStrategy, ClientError> {
    let strategy = group
        .get("strategy")
        .ok_or(ClientError::UnsupportedFeature)?;
    fields(strategy, &["type", "settings"])?;
    check_empty(strategy, "settings", |settings| {
        settings.as_object().is_some_and(serde_json::Map::is_empty)
    })?;
    let fallback = optional_str(group, "fallbackTag")?
        .filter(|tag| !tag.is_empty())
        .map(str::to_owned);
    match optional_str(strategy, "type")? {
        Some("roundRobin") if fallback.is_none() => Ok(GraphStrategy::RoundRobin),
        Some("leastPing") => Ok(GraphStrategy::LeastPing { fallback }),
        _ => Err(ClientError::UnsupportedFeature),
    }
}

fn parse_observatory(
    document: &Value,
    outbounds: &[GraphOutbound],
) -> Result<Option<GraphObservatory>, ClientError> {
    let Some(observer) = document.get("observatory") else {
        return Ok(None);
    };
    fields(
        observer,
        &[
            "subjectSelector",
            "probeUrl",
            "probeURL",
            "probeInterval",
            "enableConcurrency",
        ],
    )?;
    let selectors = strings(observer, "subjectSelector")?;
    let members = outbounds
        .iter()
        .filter(|outbound| {
            selectors
                .iter()
                .any(|prefix| outbound.tag.starts_with(prefix))
        })
        .map(|outbound| outbound.tag.clone())
        .collect();
    let probe_url = observer_probe_url(observer)?
        .filter(|url| !url.is_empty())
        .unwrap_or("https://www.google.com/generate_204");
    let enable_concurrency = observer
        .get("enableConcurrency")
        .map(|value| value.as_bool().ok_or(ClientError::InvalidSubscription))
        .transpose()?
        .unwrap_or(false);
    Ok(Some(GraphObservatory {
        members,
        probe_url: Secret::new(probe_url),
        interval: parse_interval(optional_str(observer, "probeInterval")?)?,
        enable_concurrency,
    }))
}

fn observer_probe_url(observer: &Value) -> Result<Option<&str>, ClientError> {
    match (
        optional_str(observer, "probeUrl")?,
        optional_str(observer, "probeURL")?,
    ) {
        (Some(_), Some(_)) => Err(ClientError::InvalidSubscription),
        (url, None) | (None, url) => Ok(url),
    }
}

fn parse_interval(value: Option<&str>) -> Result<Duration, ClientError> {
    let Some(value) = value else {
        return Ok(Duration::from_secs(10));
    };
    let mut remaining = value.strip_prefix('+').unwrap_or(value);
    if remaining == "0" {
        return Ok(Duration::from_secs(10));
    }
    if remaining.is_empty() {
        return Err(ClientError::InvalidSubscription);
    }
    let mut nanos = 0u128;
    while !remaining.is_empty() {
        let number_end = remaining
            .find(|value: char| !value.is_ascii_digit() && value != '.')
            .ok_or(ClientError::InvalidSubscription)?;
        let number = &remaining[..number_end];
        let (scale, rest) = duration_unit(&remaining[number_end..])?;
        nanos = nanos
            .checked_add(duration_number(number, scale)?)
            .ok_or(ClientError::InvalidSubscription)?;
        if nanos > 86_400_000_000_000 {
            return Err(ClientError::UnsupportedFeature);
        }
        remaining = rest;
    }
    if nanos == 0 {
        return Ok(Duration::from_secs(10));
    }
    Ok(Duration::from_nanos(
        u64::try_from(nanos).map_err(|_| ClientError::InvalidSubscription)?,
    ))
}

fn duration_unit(value: &str) -> Result<(u128, &str), ClientError> {
    for (unit, scale) in [
        ("ns", 1),
        ("us", 1_000),
        ("µs", 1_000),
        ("μs", 1_000),
        ("ms", 1_000_000),
        ("s", 1_000_000_000),
        ("m", 60_000_000_000),
        ("h", 3_600_000_000_000),
    ] {
        if let Some(rest) = value.strip_prefix(unit) {
            return Ok((scale, rest));
        }
    }
    Err(ClientError::InvalidSubscription)
}

fn duration_number(value: &str, scale: u128) -> Result<u128, ClientError> {
    let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
    if whole.is_empty() && fraction.is_empty()
        || !whole
            .bytes()
            .chain(fraction.bytes())
            .all(|value| value.is_ascii_digit())
    {
        return Err(ClientError::InvalidSubscription);
    }
    if fraction.len() > 18 {
        return Err(ClientError::UnsupportedFeature);
    }
    let whole = if whole.is_empty() {
        0
    } else {
        whole
            .parse::<u128>()
            .map_err(|_| ClientError::InvalidSubscription)?
    };
    let fraction = if fraction.is_empty() {
        0
    } else {
        fraction
            .parse::<u128>()
            .map_err(|_| ClientError::InvalidSubscription)?
            .checked_mul(scale)
            .ok_or(ClientError::InvalidSubscription)?
            / 10u128.pow(fraction.len() as u32)
    };
    whole
        .checked_mul(scale)
        .and_then(|whole| whole.checked_add(fraction))
        .ok_or(ClientError::InvalidSubscription)
}

fn strings<'a>(document: &'a Value, key: &str) -> Result<Vec<&'a str>, ClientError> {
    array(document, key)?
        .iter()
        .map(|value| value.as_str().ok_or(ClientError::InvalidSubscription))
        .collect()
}

fn parse_rule(rule: &Value, inbound: Option<&str>) -> Result<GraphRule, ClientError> {
    fields(
        rule,
        &[
            "type",
            "domain",
            "ip",
            "network",
            "inboundTag",
            "outboundTag",
            "balancerTag",
        ],
    )?;
    check_empty(rule, "type", |kind| kind.as_str() == Some("field"))?;
    let inbound_tags = strings(rule, "inboundTag")?;
    if inbound_tags.iter().any(|tag| Some(*tag) != inbound) {
        return Err(ClientError::UnsupportedFeature);
    }
    let mut conditions = Vec::new();
    if rule.get("domain").is_some() {
        conditions.push(parse_domains(rule)?);
    }
    if rule.get("ip").is_some() {
        conditions.push(GraphMatch::Cidr(
            strings(rule, "ip")?
                .into_iter()
                .map(cidr)
                .collect::<Result<_, _>>()?,
        ));
    }
    if let Some(network) = rule.get("network") {
        conditions.push(parse_network(network)?);
    }
    let condition = match conditions.len() {
        0 => GraphMatch::AnyDestination,
        1 => conditions.remove(0),
        _ => GraphMatch::All(conditions),
    };
    let target = match (
        optional_str(rule, "outboundTag")?,
        optional_str(rule, "balancerTag")?,
    ) {
        (Some(tag), None) => GraphTarget::Outbound(tag.to_owned()),
        (None, Some(tag)) => GraphTarget::Group(tag.to_owned()),
        _ => return Err(ClientError::InvalidSubscription),
    };
    Ok(GraphRule { condition, target })
}

fn parse_domains(rule: &Value) -> Result<GraphMatch, ClientError> {
    let mut exact = Vec::new();
    let mut suffix = Vec::new();
    for value in strings(rule, "domain")? {
        if let Some(name) = value.strip_prefix("full:") {
            exact.push(endpoint_host(name)?);
        } else if let Some(name) = value.strip_prefix("domain:") {
            suffix.push(endpoint_host(name)?);
        } else {
            return Err(ClientError::UnsupportedFeature);
        }
    }
    match (exact.is_empty(), suffix.is_empty()) {
        (false, true) => Ok(GraphMatch::Domain(exact)),
        (true, false) => Ok(GraphMatch::DomainSuffix(suffix)),
        (false, false) => Ok(GraphMatch::Any(vec![
            GraphMatch::Domain(exact),
            GraphMatch::DomainSuffix(suffix),
        ])),
        (true, true) => Err(ClientError::InvalidSubscription),
    }
}

fn parse_network(value: &Value) -> Result<GraphMatch, ClientError> {
    let value = value.as_str().ok_or(ClientError::InvalidSubscription)?;
    let mut networks = Vec::new();
    for network in value.split(',').map(str::trim) {
        let network = match network {
            "tcp" => GraphNetwork::Tcp,
            "udp" => GraphNetwork::Udp,
            _ => return Err(ClientError::UnsupportedFeature),
        };
        if !networks.contains(&network) {
            networks.push(network);
        }
    }
    Ok(GraphMatch::Network(networks))
}

fn cidr(value: &str) -> Result<String, ClientError> {
    let (address, prefix) = match value.split_once('/') {
        Some((address, prefix)) => (address.parse::<IpAddr>(), Some(prefix.parse::<u32>())),
        None => (value.parse::<IpAddr>(), None),
    };
    let address = address.map_err(|_| ClientError::UnsupportedFeature)?;
    let prefix = prefix
        .transpose()
        .map_err(|_| ClientError::InvalidSubscription)?;
    match address {
        IpAddr::V4(address) => {
            let prefix = prefix.unwrap_or(32);
            if prefix > 32 {
                return Err(ClientError::InvalidSubscription);
            }
            let mask = u32::MAX.checked_shl(32 - prefix).unwrap_or(0);
            Ok(format!(
                "{}/{}",
                Ipv4Addr::from(u32::from(address) & mask),
                prefix
            ))
        }
        IpAddr::V6(address) => {
            let prefix = prefix.unwrap_or(128);
            if prefix > 128 {
                return Err(ClientError::InvalidSubscription);
            }
            let mask = u128::MAX.checked_shl(128 - prefix).unwrap_or(0);
            Ok(format!(
                "{}/{}",
                Ipv6Addr::from(u128::from(address) & mask),
                prefix
            ))
        }
    }
}

fn graph_profile(document: &Value, graph: ProxyGraph) -> Result<ProxyProfile, ClientError> {
    let representative = graph
        .outbounds
        .iter()
        .find_map(|outbound| match &outbound.kind {
            GraphOutboundKind::Proxy(profile) => Some(profile),
            _ => None,
        })
        .ok_or(ClientError::UnsupportedFeature)?;
    let host = representative.host.clone();
    let port = representative.port;
    let name = optional_str(document, "remarks")?.unwrap_or("Xray configuration");
    let mut profile = make_profile(
        name,
        &host,
        port,
        ProxyProtocol::Graph(Box::new(graph)),
        ProxyTransport::Tcp,
        ProxySecurity::None,
    )?;
    profile.source = Some(Secret::new(
        serde_json::to_string_pretty(document).map_err(|_| ClientError::InvalidSubscription)?,
    ));
    Ok(profile)
}
