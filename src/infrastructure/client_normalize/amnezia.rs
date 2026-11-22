use std::borrow::Cow;

use base64::{
    Engine as _,
    engine::general_purpose::{URL_SAFE, URL_SAFE_NO_PAD},
};
use flate2::{Decompress, FlushDecompress, Status};
use serde_json::Value;
use vpn_application::MAX_SUBSCRIPTION_BYTES;
use zeroize::Zeroizing;

use super::{ClientError, ProxyProfile, bracket_host, endpoint_host, json, wireguard};

const AWG_FIELDS: &[&str] = &[
    "Jc",
    "Jmin",
    "Jmax",
    "S1",
    "S2",
    "S3",
    "S4",
    "H1",
    "H2",
    "H3",
    "H4",
    "I1",
    "I2",
    "I3",
    "I4",
    "I5",
    "HeaderProtectionKey",
    "ContentPaddingAddition",
    "RekeyAfterTime",
    "RekeyTimeout",
    "RejectAfterTime",
    "KeepaliveTimeout",
    "MaxHandshakeAttempts",
    "RandomTrailers",
    "DisableCookies",
];

pub(super) fn parse_link(encoded: &str) -> Result<ProxyProfile, ClientError> {
    let decoded = Zeroizing::new(
        URL_SAFE_NO_PAD
            .decode(encoded)
            .or_else(|_| URL_SAFE.decode(encoded))
            .map_err(|_| ClientError::InvalidSubscription)?,
    );
    if let Ok(text) = std::str::from_utf8(&decoded) {
        let text = text.trim().trim_start_matches('\u{feff}').trim();
        if is_client_document(text) {
            return parse_document(text);
        }
    }
    let inflated = inflate_qt(&decoded)?;
    let text = std::str::from_utf8(&inflated)
        .map_err(|_| ClientError::InvalidSubscription)?
        .trim()
        .trim_start_matches('\u{feff}')
        .trim();
    parse_document(text)
}

fn inflate_qt(decoded: &[u8]) -> Result<Zeroizing<Vec<u8>>, ClientError> {
    let (header, stream) = decoded
        .split_first_chunk::<4>()
        .ok_or(ClientError::InvalidSubscription)?;
    let length = u32::from_be_bytes(*header) as usize;
    if length > MAX_SUBSCRIPTION_BYTES {
        return Err(ClientError::PayloadTooLarge);
    }
    let mut output = Zeroizing::new(vec![0; length + 1]);
    let mut decoder = Decompress::new(true);
    let status = decoder
        .decompress(stream, &mut output, FlushDecompress::Finish)
        .map_err(|_| ClientError::InvalidSubscription)?;
    if status != Status::StreamEnd
        || decoder.total_out() != length as u64
        || decoder.total_in() != stream.len() as u64
    {
        return Err(ClientError::InvalidSubscription);
    }
    output.truncate(length);
    Ok(output)
}

fn is_client_document(text: &str) -> bool {
    text.starts_with('{') || wireguard::recognizes(text)
}

fn parse_document(text: &str) -> Result<ProxyProfile, ClientError> {
    if wireguard::recognizes(text) {
        return wireguard::parse(text);
    }
    if text.starts_with('{') {
        return parse_bundle(&json::parse_value(text)?);
    }
    Err(ClientError::UnsupportedFormat)
}

pub(super) fn parse_bundle(bundle: &Value) -> Result<ProxyProfile, ClientError> {
    if bundle
        .get("send_payload")
        .is_some_and(|value| !value.is_null())
    {
        return Err(ClientError::UnsupportedFeature);
    }
    let (container, awg) = select_container(bundle)?;
    let protocol = select_protocol(container, awg)?;
    let last = read_last_config(protocol)?;
    let mut profile = build_wireguard(bundle, protocol, &last, awg)?;
    if let Some(name) = json::optional_str(bundle, "description")?
        && !name.is_empty()
        && name.len() <= 256
        && !name.chars().any(char::is_control)
    {
        profile.name = name.to_owned();
    }
    Ok(profile)
}

fn select_container(bundle: &Value) -> Result<(&Value, bool), ClientError> {
    let containers = bundle
        .get("containers")
        .and_then(Value::as_array)
        .ok_or(ClientError::UnsupportedFormat)?;
    let preferred = json::optional_str(bundle, "defaultContainer")?.filter(|name| !name.is_empty());
    let mut matching = containers.iter().filter(|container| {
        let name = container.get("container").and_then(Value::as_str);
        if let Some(preferred) = preferred {
            return name == Some(preferred);
        }
        name.and_then(container_kind).is_some()
    });
    let selected = matching.next().ok_or(ClientError::UnsupportedFeature)?;
    if matching.next().is_some() {
        return Err(ClientError::UnsupportedFeature);
    }
    let kind = container_kind(json::string(selected, "container")?)
        .ok_or(ClientError::UnsupportedFeature)?;
    Ok((selected, kind))
}

fn container_kind(name: &str) -> Option<bool> {
    match name {
        "amnezia-wireguard" => Some(false),
        "amnezia-awg" | "amnezia-awg2" => Some(true),
        _ => None,
    }
}

fn select_protocol(container: &Value, awg: bool) -> Result<&Value, ClientError> {
    if awg {
        return container
            .get("awg")
            .or_else(|| container.get("awg2"))
            .ok_or(ClientError::InvalidSubscription);
    }
    container
        .get("wireguard")
        .ok_or(ClientError::InvalidSubscription)
}

fn read_last_config(protocol: &Value) -> Result<Cow<'_, Value>, ClientError> {
    match protocol.get("last_config") {
        Some(Value::String(text)) if !text.is_empty() => json::parse_value(text).map(Cow::Owned),
        Some(value @ Value::Object(_)) => Ok(Cow::Borrowed(value)),
        Some(_) | None => Err(ClientError::UnsupportedFeature),
    }
}

fn build_wireguard(
    bundle: &Value,
    protocol: &Value,
    client: &Value,
    awg: bool,
) -> Result<ProxyProfile, ClientError> {
    let raw = json::optional_str(client, "config")?.filter(|text| !text.trim().is_empty());
    let (mut interface, mut peer) = match raw {
        Some(text) if wireguard::recognizes(text) => wireguard::sections(text)?,
        Some(_) => return Err(ClientError::UnsupportedFormat),
        None => Default::default(),
    };
    fill_config_fields(&mut interface, &mut peer, client)?;
    let endpoint = (!peer.contains_key("endpoint"))
        .then(|| client_endpoint(bundle, protocol, client))
        .transpose()?;
    if let Some(endpoint) = &endpoint {
        peer.insert("endpoint".into(), endpoint);
    }
    let allowed = (!peer.contains_key("allowedips"))
        .then(|| allowed_ips(client))
        .transpose()?;
    if let Some(allowed) = &allowed {
        peer.insert("allowedips".into(), allowed);
    }
    let dns = bundle_dns(bundle, interface.remove("dns"))?;
    if !dns.is_empty() {
        interface.insert("dns".into(), &dns);
    }
    let fields = awg_fields(client, protocol)?;
    for (key, value) in &fields {
        interface.insert(key.to_ascii_lowercase(), value);
    }
    wireguard::parse_sections(interface, peer, awg)
}

fn fill_config_fields<'a>(
    interface: &mut wireguard::Section<'a>,
    peer: &mut wireguard::Section<'a>,
    client: &'a Value,
) -> Result<(), ClientError> {
    for (name, key) in [
        ("privatekey", "client_priv_key"),
        ("address", "client_ip"),
        ("mtu", "mtu"),
    ] {
        fill_from_json(interface, name, client, key)?;
    }
    for (name, key) in [
        ("publickey", "server_pub_key"),
        ("presharedkey", "psk_key"),
        ("persistentkeepalive", "persistent_keep_alive"),
    ] {
        fill_from_json(peer, name, client, key)?;
    }
    Ok(())
}

fn fill_from_json<'a>(
    section: &mut wireguard::Section<'a>,
    name: &str,
    client: &'a Value,
    key: &str,
) -> Result<(), ClientError> {
    if let Some(value) = json::optional_str(client, key)?.filter(|value| !value.trim().is_empty()) {
        section.entry(name.to_owned()).or_insert(value.trim());
    }
    Ok(())
}

fn client_endpoint(
    bundle: &Value,
    protocol: &Value,
    client: &Value,
) -> Result<String, ClientError> {
    let host = json::optional_str(client, "hostName")?
        .filter(|host| !host.is_empty())
        .or(json::optional_str(bundle, "hostName")?)
        .ok_or(ClientError::InvalidSubscription)?;
    let host = endpoint_host(host)?;
    let port = json::port(client, "port").or_else(|_| json::port(protocol, "port"))?;
    let mut endpoint = bracket_host(&host);
    endpoint.push(':');
    endpoint.push_str(&port.to_string());
    Ok(endpoint)
}

fn allowed_ips(client: &Value) -> Result<String, ClientError> {
    let Some(ips) = client.get("allowed_ips") else {
        return Ok("0.0.0.0/0, ::/0".to_owned());
    };
    let ips = ips.as_array().ok_or(ClientError::InvalidSubscription)?;
    let ips = ips
        .iter()
        .map(|ip| ip.as_str().ok_or(ClientError::InvalidSubscription))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ips.join(", "))
}

fn bundle_dns(bundle: &Value, configured: Option<&str>) -> Result<String, ClientError> {
    let dns_field = |key| {
        json::optional_str(bundle, key)
            .map(|value| value.map(str::trim).filter(|dns| !dns.is_empty()))
    };
    let primary = dns_field("dns1")?;
    let secondary = dns_field("dns2")?;
    let Some(configured) = configured.filter(|dns| !dns.trim().is_empty()) else {
        return Ok(primary
            .into_iter()
            .chain(secondary)
            .collect::<Vec<_>>()
            .join(", "));
    };
    let servers = configured
        .split(',')
        .map(str::trim)
        .filter_map(|entry| match entry {
            "$PRIMARY_DNS" => primary,
            "$SECONDARY_DNS" => secondary,
            _ => Some(entry),
        })
        .collect::<Vec<_>>();
    if servers.is_empty() {
        return Err(ClientError::InvalidSubscription);
    }
    Ok(servers.join(", "))
}

fn awg_fields<'a>(
    client: &'a Value,
    protocol: &'a Value,
) -> Result<Vec<(&'static str, Cow<'a, str>)>, ClientError> {
    let mut fields = Vec::new();
    for &key in AWG_FIELDS {
        let value = client.get(key).or_else(|| protocol.get(key));
        let value = match value {
            Some(Value::String(value)) if !value.trim().is_empty() => Cow::Borrowed(value.trim()),
            Some(Value::Number(value)) => Cow::Owned(value.to_string()),
            Some(Value::Bool(value)) => Cow::Owned(value.to_string()),
            Some(Value::String(_)) | Some(Value::Null) | None => continue,
            _ => return Err(ClientError::InvalidSubscription),
        };
        fields.push((key, value));
    }
    Ok(fields)
}
