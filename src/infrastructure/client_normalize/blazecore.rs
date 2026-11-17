use serde_json::Value;

use super::json::{fields, flag, optional_str, string};
use super::{
    ClientError, ProxyProfile, ProxyProtocol, ProxySecurity, ProxyTransport, make_profile,
    proxy_auth, shadowsocks, trojan, vless, vmess,
};

pub(super) fn parse(value: &Value) -> Result<ProxyProfile, ClientError> {
    let upstream = if let Some(rules) = value.get("rules") {
        listener_upstream(value, rules)?
    } else {
        value
    };
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

fn listener_upstream<'a>(value: &Value, rules: &'a Value) -> Result<&'a Value, ClientError> {
    fields(value, &["address", "protocol", "rules"])?;
    string(value, "address")?;
    let listener = value
        .get("protocol")
        .ok_or(ClientError::InvalidSubscription)?;
    fields(listener, &["type"])?;
    if string(listener, "type")? != "mixed" {
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
