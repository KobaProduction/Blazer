use std::{collections::BTreeMap, fmt::Write as _, net::IpAddr};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::{Map, Value, json};

use super::{ClientError, ProxyProfile, ProxyProtocol, ProxySecurity, Secret, make_profile};
use crate::native_profile;

pub(super) type Section<'a> = BTreeMap<String, &'a str>;

pub(super) fn recognizes(text: &str) -> bool {
    text.lines()
        .map(clean_line)
        .find(|line| !line.is_empty())
        .is_some_and(|line| line.eq_ignore_ascii_case("[interface]"))
}

pub(super) fn parse(text: &str) -> Result<ProxyProfile, ClientError> {
    let (interface, peer) = sections(text)?;
    let mut profile = parse_sections(interface, peer, false)?;
    profile.source = Some(Secret::new(text));
    Ok(profile)
}

pub(super) fn parse_sections(
    mut interface: Section<'_>,
    mut peer: Section<'_>,
    amnezia_profile: bool,
) -> Result<ProxyProfile, ClientError> {
    let (host, port) = native_profile::endpoint(take_required(&mut peer, "endpoint")?)?;
    let dns = interface
        .remove("dns")
        .map(dns_servers)
        .transpose()?
        .unwrap_or_default();
    let mut protocol = json!({
        "type": "wireguard",
        "private_key": take_required(&mut interface, "privatekey")?,
        "public_key": take_required(&mut peer, "publickey")?,
        "addresses": prefixes(take_required(&mut interface, "address")?),
        "allowed_ips": prefixes(take_required(&mut peer, "allowedips")?),
    });
    if let Some(secret) = peer.remove("presharedkey") {
        protocol["pre_shared_key"] = Value::from(secret);
    }
    let range_keepalive = keepalive(peer.remove("persistentkeepalive"), &mut protocol)?;
    copy_u16(&mut interface, "mtu", &mut protocol, "mtu")?;
    if !peer.is_empty() {
        return Err(ClientError::UnsupportedFeature);
    }
    if amnezia_profile || !interface.is_empty() || range_keepalive.is_some() {
        protocol["amnezia"] = amnezia(interface, range_keepalive)?;
    }
    let (proxy, transport) = native_profile::parse(json!({"protocol": protocol}), dns)?;
    make_profile(
        "",
        &host,
        port,
        ProxyProtocol::Native(proxy),
        transport,
        ProxySecurity::None,
    )
}

pub(super) fn sections(text: &str) -> Result<(Section<'_>, Section<'_>), ClientError> {
    let mut interface = Section::new();
    let mut peer = Section::new();
    let mut seen_peer = false;
    let mut current = &mut interface;
    for line in text
        .lines()
        .map(clean_line)
        .filter(|line| !line.is_empty())
        .skip(1)
    {
        if line.starts_with('[') {
            if seen_peer || !line.eq_ignore_ascii_case("[peer]") {
                return Err(ClientError::UnsupportedFeature);
            }
            seen_peer = true;
            current = &mut peer;
            continue;
        }
        let (key, value) = line
            .split_once('=')
            .ok_or(ClientError::InvalidSubscription)?;
        if current
            .insert(key.trim().to_ascii_lowercase(), value.trim())
            .is_some()
        {
            return Err(ClientError::InvalidSubscription);
        }
    }
    Ok((interface, peer))
}

fn clean_line(line: &str) -> &str {
    line.split(['#', ';']).next().unwrap_or_default().trim()
}

fn take_required<'a>(section: &mut Section<'a>, key: &str) -> Result<&'a str, ClientError> {
    section
        .remove(key)
        .filter(|value| !value.is_empty())
        .ok_or(ClientError::InvalidSubscription)
}

fn copy_u16(
    section: &mut Section<'_>,
    source: &str,
    target: &mut Value,
    name: &str,
) -> Result<(), ClientError> {
    if let Some(value) = section.remove(source) {
        target[name] = Value::from(
            value
                .parse::<u16>()
                .map_err(|_| ClientError::InvalidSubscription)?,
        );
    }
    Ok(())
}

fn prefixes(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .map(|value| match value.parse::<IpAddr>() {
            Ok(IpAddr::V4(address)) => format!("{address}/32"),
            Ok(IpAddr::V6(address)) => format!("{address}/128"),
            Err(_) => value.to_owned(),
        })
        .collect()
}

fn dns_servers(value: &str) -> Result<Vec<IpAddr>, ClientError> {
    value
        .split(',')
        .map(|value| {
            value
                .trim()
                .parse()
                .map_err(|_| ClientError::UnsupportedFeature)
        })
        .collect()
}

fn keepalive(value: Option<&str>, protocol: &mut Value) -> Result<Option<Value>, ClientError> {
    let Some(value) = value else { return Ok(None) };
    if value.eq_ignore_ascii_case("off") {
        return Ok(None);
    }
    if value.contains('-') {
        return parse_range(value).map(Some);
    }
    protocol["persistent_keepalive"] = Value::from(
        value
            .parse::<u16>()
            .map_err(|_| ClientError::InvalidSubscription)?,
    );
    Ok(None)
}

fn amnezia(interface: Section<'_>, keepalive: Option<Value>) -> Result<Value, ClientError> {
    let mut profile = Map::new();
    profile.insert("version".into(), Value::from("3.1"));
    if let Some(value) = keepalive {
        profile.insert("persistent_keepalive_interval".into(), value);
    }
    for (key, value) in interface {
        let key = canonical_amnezia_key(&key);
        let field = amnezia_field(key, value)?;
        if profile.insert(key.to_owned(), field).is_some() {
            return Err(ClientError::InvalidSubscription);
        }
    }
    Ok(Value::Object(profile))
}

fn canonical_amnezia_key(key: &str) -> &str {
    match key {
        "headerprotectionkey" => "header_protection_key",
        "contentpaddingaddition" => "content_padding_addition",
        "rekeyaftertime" => "rekey_after_time",
        "rekeytimeout" => "rekey_timeout",
        "rejectaftertime" => "reject_after_time",
        "keepalivetimeout" => "keepalive_timeout",
        "maxhandshakeattempts" => "max_handshake_attempts",
        "randomtrailers" => "random_trailers",
        "disablecookies" => "disable_cookies",
        _ => key,
    }
}

fn amnezia_field(key: &str, value: &str) -> Result<Value, ClientError> {
    let invalid = |_| ClientError::InvalidSubscription;
    let field = match key {
        "jc" | "jmin" | "jmax" | "s1" | "s2" | "s3" | "s4" => {
            Value::from(value.parse::<u16>().map_err(invalid)?)
        }
        "h1"
        | "h2"
        | "h3"
        | "h4"
        | "content_padding_addition"
        | "rekey_after_time"
        | "rekey_timeout"
        | "reject_after_time"
        | "keepalive_timeout"
        | "max_handshake_attempts"
        | "persistent_keepalive_interval" => parse_range(value)?,
        "i1" | "i2" | "i3" | "i4" | "i5" => Value::from(value),
        "header_protection_key" => header_protection_key(value)?,
        "random_trailers" | "disable_cookies" => match value.to_ascii_lowercase().as_str() {
            "true" | "on" | "1" => Value::from(true),
            "false" | "off" | "0" => Value::from(false),
            _ => return Err(ClientError::InvalidSubscription),
        },
        _ => return Err(ClientError::UnsupportedFeature),
    };
    Ok(field)
}

fn header_protection_key(value: &str) -> Result<Value, ClientError> {
    let mut decoded = zeroize::Zeroizing::new([0_u8; 33]);
    if STANDARD.decode_slice(value, &mut decoded[..]).ok() != Some(32) {
        return Err(ClientError::InvalidSubscription);
    }
    let mut encoded = String::with_capacity(64);
    for byte in &decoded[..32] {
        write!(&mut encoded, "{byte:02x}").expect("writing to a String succeeds");
    }
    Ok(Value::from(encoded))
}

fn parse_range(value: &str) -> Result<Value, ClientError> {
    let parse = |value: &str| {
        value
            .trim()
            .parse::<u32>()
            .map_err(|_| ClientError::InvalidSubscription)
    };
    let Some((min, max)) = value.split_once('-') else {
        return Ok(Value::from(parse(value)?));
    };
    let (min, max) = (parse(min)?, parse(max)?);
    if min > max {
        return Err(ClientError::InvalidSubscription);
    }
    Ok(Value::from(format!("{min}-{max}")))
}
