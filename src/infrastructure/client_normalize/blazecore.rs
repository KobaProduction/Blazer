use std::collections::BTreeSet;

use serde_json::Value;

use super::json::{fields, flag, optional_str, string};
use super::{
    ClientError, MAX_PROFILES, ProxyProfile, ProxyProtocol, ProxySecurity, ProxyTransport,
    make_profile, proxy_auth, shadowsocks, trojan, vless, vmess,
};

pub(super) fn parse(value: &Value) -> Result<ProxyProfile, ClientError> {
    let upstream = if let Some(rules) = value.get("rules") {
        listener_upstream(value, rules)?
    } else {
        value
    };
    match parse_simple(upstream) {
        Err(ClientError::UnsupportedFeature) => parse_native(upstream),
        result => result,
    }
}

fn parse_simple(upstream: &Value) -> Result<ProxyProfile, ClientError> {
    fields(upstream, &["address", "protocol"])?;
    let (host, port) = string(upstream, "address")?
        .rsplit_once(':')
        .ok_or(ClientError::InvalidSubscription)?;
    let port = port
        .parse::<u16>()
        .ok()
        .filter(|port| *port != 0)
        .ok_or(ClientError::InvalidSubscription)?;
    let protocol = upstream
        .get("protocol")
        .ok_or(ClientError::InvalidSubscription)?;
    make_profile(
        "",
        host,
        port,
        parse_protocol(protocol)?,
        ProxyTransport::Tcp,
        ProxySecurity::None,
    )
}

fn parse_native(upstream: &Value) -> Result<ProxyProfile, ClientError> {
    let (host, port) = crate::native_profile::endpoint(string(upstream, "address")?)?;
    let mut settings = upstream
        .as_object()
        .ok_or(ClientError::InvalidSubscription)?
        .clone();
    settings.remove("address");
    let name = match settings.remove("name") {
        Some(Value::String(name)) => name,
        Some(_) => return Err(ClientError::InvalidSubscription),
        None => String::new(),
    };
    let dns = settings
        .remove("dns_servers")
        .map(serde_json::from_value)
        .transpose()
        .map_err(|_| ClientError::InvalidSubscription)?
        .unwrap_or_default();
    let (protocol, transport) = crate::native_profile::parse(Value::Object(settings), dns)?;
    make_profile(
        &name,
        &host,
        port,
        ProxyProtocol::Native(protocol),
        transport,
        ProxySecurity::None,
    )
}

fn listener_upstream<'a>(value: &Value, rules: &'a Value) -> Result<&'a Value, ClientError> {
    let upstream = listener_chain(value, rules)?;
    match upstream.as_array() {
        Some(items) => match items.as_slice() {
            [only] => Ok(only),
            _ => Err(ClientError::UnsupportedFeature),
        },
        None => Ok(upstream),
    }
}

fn listener_chain<'a>(value: &Value, rules: &'a Value) -> Result<&'a Value, ClientError> {
    fields(value, &["address", "protocol", "rules"])?;
    string(value, "address")?;
    let listener = value
        .get("protocol")
        .ok_or(ClientError::InvalidSubscription)?;
    fields(listener, &["type"])?;
    if !matches!(string(listener, "type")?, "mixed" | "socks" | "http") {
        return Err(ClientError::UnsupportedFeature);
    }
    let Some([rule]) = rules.as_array().map(Vec::as_slice) else {
        return Err(ClientError::UnsupportedFeature);
    };
    fields(rule, &["action", "masks", "client_chain"])?;
    let masks = rule
        .get("masks")
        .and_then(Value::as_array)
        .ok_or(ClientError::InvalidSubscription)?;
    if masks.len() != 2
        || !masks.iter().any(|value| value == "0.0.0.0/0")
        || !masks.iter().any(|value| value == "::/0")
        || string(rule, "action")? != "allow"
    {
        return Err(ClientError::UnsupportedFeature);
    }
    rule.get("client_chain")
        .ok_or(ClientError::InvalidSubscription)
}

pub(super) fn expand_client_groups(items: &[Value]) -> Result<Option<Vec<Value>>, ClientError> {
    if !items.iter().any(|item| item.get("client_group").is_some()) {
        return Ok(None);
    }
    let mut expanded = Vec::new();
    let mut names = BTreeSet::new();
    for group in items
        .iter()
        .filter(|item| item.get("client_group").is_some())
    {
        let name = expand_group(group, &mut expanded)?;
        if !names.insert(name) {
            return Err(ClientError::InvalidSubscription);
        }
    }
    for item in items
        .iter()
        .filter(|item| item.get("client_group").is_none())
    {
        let Some(rules) = item.get("rules") else {
            return Err(ClientError::UnsupportedFeature);
        };
        let chain = listener_chain(item, rules)?;
        let name = chain.as_str().ok_or(ClientError::UnsupportedFeature)?;
        if !names.contains(name) {
            return Err(ClientError::UnsupportedFeature);
        }
    }
    Ok(Some(expanded))
}

fn expand_group<'a>(group: &'a Value, expanded: &mut Vec<Value>) -> Result<&'a str, ClientError> {
    fields(group, &["client_group", "client_chains"])?;
    let name = string(group, "client_group")?;
    let chains = group
        .get("client_chains")
        .and_then(Value::as_array)
        .filter(|chains| !chains.is_empty())
        .ok_or(ClientError::InvalidSubscription)?;
    if chains.len() > MAX_PROFILES - expanded.len() {
        return Err(ClientError::PayloadTooLarge);
    }
    for chain in chains {
        let endpoint = match chain.get("chain").and_then(Value::as_array) {
            Some(hops) => {
                fields(chain, &["chain"])?;
                let [endpoint] = hops.as_slice() else {
                    return Err(ClientError::UnsupportedFeature);
                };
                endpoint
            }
            None => chain,
        };
        fields(
            endpoint,
            &[
                "address",
                "name",
                "protocol",
                "transport",
                "tcp_settings",
                "quic_settings",
                "dns_servers",
            ],
        )?;
        expanded.push(endpoint.clone());
    }
    Ok(name)
}

fn parse_protocol(protocol: &Value) -> Result<ProxyProtocol, ClientError> {
    let kind = string(protocol, "type")?;
    match kind {
        "socks" | "http" => {
            fields(protocol, &["type", "username", "password"])?;
            proxy_auth(
                kind,
                optional_str(protocol, "username")?,
                optional_str(protocol, "password")?,
            )
        }
        "vless" => {
            fields(protocol, &["type", "user_id", "vision", "encryption"])?;
            let flow = if flag(protocol, "vision")? {
                "xtls-rprx-vision"
            } else {
                ""
            };
            vless(
                string(protocol, "user_id")?,
                flow,
                optional_str(protocol, "encryption")?.unwrap_or("none"),
            )
        }
        "vmess" => {
            fields(protocol, &["type", "user_id", "cipher"])?;
            vmess(string(protocol, "user_id")?, string(protocol, "cipher")?)
        }
        "trojan" => {
            fields(protocol, &["type", "password"])?;
            trojan(string(protocol, "password")?)
        }
        "shadowsocks" => {
            fields(protocol, &["type", "cipher", "password"])?;
            shadowsocks(string(protocol, "cipher")?, string(protocol, "password")?)
        }
        _ => Err(ClientError::UnsupportedFeature),
    }
}
