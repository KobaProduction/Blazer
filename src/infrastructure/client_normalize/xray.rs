use serde_json::Value;

use super::json::{
    check_empty, fields, flag, host_header, optional_str, port, string, tls_alpn, zero,
};
use super::{
    ClientError, ProxyProfile, ProxyProtocol, ProxySecurity, ProxyTransport, make_profile,
    proxy_auth, reality, shadowsocks, trojan, verified_tls, vless, vmess, with_tls_fingerprint,
    xhttp, xray_websocket,
};

pub(super) fn parse_outbound(outbound: &Value) -> Result<ProxyProfile, ClientError> {
    fields(
        outbound,
        &["protocol", "tag", "settings", "streamSettings", "mux"],
    )?;
    if let Some(mux) = outbound.get("mux") {
        fields(mux, &["enabled"])?;
        if flag(mux, "enabled")? {
            return Err(ClientError::UnsupportedFeature);
        }
    }
    let kind = string(outbound, "protocol")?;
    let settings = outbound
        .get("settings")
        .ok_or(ClientError::InvalidSubscription)?;
    let server = endpoint(kind, settings)?;
    let host = string(server, "address")?;
    let port = port(server, "port")?;
    let protocol = parse_protocol(kind, server)?;
    let (transport, security) = parse_stream(outbound.get("streamSettings"), host)?;
    let name = optional_str(outbound, "tag")?.unwrap_or("");
    make_profile(name, host, port, protocol, transport, security)
}

pub(super) fn endpoint<'a>(kind: &str, settings: &'a Value) -> Result<&'a Value, ClientError> {
    let collection = match kind {
        "vless" | "vmess" => "vnext",
        "socks" | "http" | "trojan" | "shadowsocks" => "servers",
        _ => return Err(ClientError::UnsupportedFeature),
    };
    if settings.get(collection).is_none() {
        return Ok(settings);
    }
    fields(settings, &[collection])?;
    single(settings, collection)
}

fn single<'a>(value: &'a Value, key: &str) -> Result<&'a Value, ClientError> {
    let values = value
        .get(key)
        .and_then(Value::as_array)
        .ok_or(ClientError::InvalidSubscription)?;
    let [value] = values.as_slice() else {
        return Err(ClientError::UnsupportedFeature);
    };
    Ok(value)
}

fn profile_metadata(value: &Value) -> Result<(), ClientError> {
    check_empty(value, "level", zero)?;
    // Xray's user label has no wire semantics and may contain personal data.
    optional_str(value, "email")?;
    Ok(())
}

fn parse_protocol(kind: &str, server: &Value) -> Result<ProxyProtocol, ClientError> {
    match kind {
        "socks" | "http" => parse_proxy(kind, server),
        "vless" | "vmess" => parse_uuid_protocol(kind, server),
        "trojan" => {
            fields(server, &["address", "port", "password", "level", "email"])?;
            profile_metadata(server)?;
            trojan(string(server, "password")?)
        }
        "shadowsocks" => {
            fields(
                server,
                &[
                    "address", "port", "method", "password", "level", "email", "ota",
                ],
            )?;
            profile_metadata(server)?;
            check_empty(server, "ota", |value| value.as_bool() == Some(false))?;
            shadowsocks(string(server, "method")?, string(server, "password")?)
        }
        _ => Err(ClientError::UnsupportedFeature),
    }
}

fn parse_proxy(kind: &str, server: &Value) -> Result<ProxyProtocol, ClientError> {
    let user = if let Some(users) = server.get("users") {
        fields(server, &["address", "port", "users"])?;
        match users.as_array().map(Vec::as_slice) {
            Some([]) => return proxy_auth(kind, None, None),
            Some([user]) => {
                fields(user, &["user", "pass", "level", "email"])?;
                user
            }
            Some(_) => return Err(ClientError::UnsupportedFeature),
            None => return Err(ClientError::InvalidSubscription),
        }
    } else {
        fields(
            server,
            &["address", "port", "user", "pass", "level", "email"],
        )?;
        server
    };
    profile_metadata(user)?;
    proxy_auth(
        kind,
        optional_str(user, "user")?,
        optional_str(user, "pass")?,
    )
}

fn parse_uuid_protocol(kind: &str, server: &Value) -> Result<ProxyProtocol, ClientError> {
    let user = if server.get("users").is_some() {
        fields(server, &["address", "port", "users"])?;
        single(server, "users")?
    } else {
        server
    };
    profile_metadata(user)?;
    let id = string(user, "id")?;
    if kind == "vless" {
        fields(
            user,
            &[
                "address",
                "port",
                "id",
                "level",
                "email",
                "encryption",
                "flow",
            ],
        )?;
        let encryption = optional_str(user, "encryption")?.unwrap_or("none");
        if encryption.is_empty() {
            return Err(ClientError::InvalidSubscription);
        }
        return vless(id, optional_str(user, "flow")?.unwrap_or(""), encryption);
    }
    fields(
        user,
        &[
            "address",
            "port",
            "id",
            "level",
            "email",
            "security",
            "alterId",
            "experiments",
        ],
    )?;
    check_empty(user, "alterId", zero)?;
    check_empty(user, "experiments", |value| value.as_str() == Some(""))?;
    vmess(id, optional_str(user, "security")?.unwrap_or("auto"))
}

fn parse_stream(
    stream: Option<&Value>,
    host: &str,
) -> Result<(ProxyTransport, ProxySecurity), ClientError> {
    let Some(stream) = stream else {
        return Ok((ProxyTransport::Tcp, ProxySecurity::None));
    };
    fields(
        stream,
        &[
            "network",
            "method",
            "security",
            "tcpSettings",
            "rawSettings",
            "wsSettings",
            "xhttpSettings",
            "splithttpSettings",
            "tlsSettings",
            "realitySettings",
        ],
    )?;
    let network = only_one(
        optional_str(stream, "network")?,
        optional_str(stream, "method")?,
    )?;
    Ok((
        parse_transport(stream, network.unwrap_or("tcp"))?,
        parse_security(stream, host)?,
    ))
}

fn parse_transport(stream: &Value, network: &str) -> Result<ProxyTransport, ClientError> {
    let tcp = only_one(stream.get("tcpSettings"), stream.get("rawSettings"))?;
    let split = only_one(stream.get("xhttpSettings"), stream.get("splithttpSettings"))?;
    if matches!(network, "xhttp" | "splithttp") {
        if tcp.is_some() || stream.get("wsSettings").is_some() {
            return Err(ClientError::UnsupportedFeature);
        }
        return parse_xhttp(split);
    }
    if split.is_some() {
        return Err(ClientError::UnsupportedFeature);
    }
    match network {
        "tcp" | "raw" | "" => {
            check_empty(stream, "wsSettings", |_| false)?;
            if let Some(tcp) = tcp {
                plain_tcp(tcp)?;
            }
            Ok(ProxyTransport::Tcp)
        }
        "ws" | "websocket" if tcp.is_none() => parse_ws(stream.get("wsSettings")),
        _ => Err(ClientError::UnsupportedFeature),
    }
}

pub(super) fn uses_xhttp(outbound: &Value) -> bool {
    let Some(stream) = outbound.get("streamSettings") else {
        return false;
    };
    let network = only_one(stream.get("network"), stream.get("method"))
        .ok()
        .flatten()
        .and_then(Value::as_str);
    matches!(network, Some("xhttp" | "splithttp"))
}

fn parse_xhttp(value: Option<&Value>) -> Result<ProxyTransport, ClientError> {
    match value {
        Some(value) => xhttp::json_transport(value, "extra"),
        None => xhttp::transport("/", None, None, Default::default()),
    }
}

fn plain_tcp(value: &Value) -> Result<(), ClientError> {
    fields(value, &["header", "acceptProxyProtocol"])?;
    check_empty(value, "acceptProxyProtocol", |option| {
        option.as_bool() == Some(false)
    })?;
    if let Some(header) = value.get("header") {
        fields(header, &["type"])?;
        check_empty(header, "type", |option| option.as_str() == Some("none"))?;
    }
    Ok(())
}

fn parse_ws(value: Option<&Value>) -> Result<ProxyTransport, ClientError> {
    let Some(value) = value else {
        return xray_websocket("/", None);
    };
    fields(
        value,
        &[
            "path",
            "host",
            "headers",
            "acceptProxyProtocol",
            "heartbeatPeriod",
        ],
    )?;
    check_empty(value, "acceptProxyProtocol", |option| {
        option.as_bool() == Some(false)
    })?;
    check_empty(value, "heartbeatPeriod", zero)?;
    let host = only_one(optional_str(value, "host")?, host_header(value)?)?;
    xray_websocket(optional_str(value, "path")?.unwrap_or("/"), host)
}

fn parse_security(stream: &Value, host: &str) -> Result<ProxySecurity, ClientError> {
    let tls = stream.get("tlsSettings");
    let reality_settings = stream.get("realitySettings");
    match optional_str(stream, "security")?.unwrap_or("none") {
        "none" | "" if tls.is_none() && reality_settings.is_none() => Ok(ProxySecurity::None),
        "tls" if reality_settings.is_none() => parse_tls(tls, host),
        "reality" if tls.is_none() => {
            parse_reality(reality_settings.ok_or(ClientError::InvalidSubscription)?)
        }
        _ => Err(ClientError::UnsupportedFeature),
    }
}

fn parse_tls(value: Option<&Value>, host: &str) -> Result<ProxySecurity, ClientError> {
    let Some(value) = value else {
        return verified_tls(host, false, super::AlpnProtocols::default());
    };
    fields(
        value,
        &["serverName", "allowInsecure", "alpn", "fingerprint"],
    )?;
    let name = optional_str(value, "serverName")?
        .filter(|name| !name.is_empty())
        .unwrap_or(host);
    with_tls_fingerprint(
        verified_tls(name, flag(value, "allowInsecure")?, tls_alpn(value)?)?,
        optional_str(value, "fingerprint")?,
    )
}

fn parse_reality(value: &Value) -> Result<ProxySecurity, ClientError> {
    fields(
        value,
        &[
            "serverName",
            "fingerprint",
            "publicKey",
            "password",
            "shortId",
            "show",
            "spiderX",
        ],
    )?;
    check_empty(value, "show", |option| option.as_bool() == Some(false))?;
    check_empty(value, "spiderX", |option| option.as_str() == Some(""))?;
    let key = only_one(
        optional_str(value, "publicKey")?,
        optional_str(value, "password")?,
    )?
    .ok_or(ClientError::InvalidSubscription)?;
    let security = reality(
        string(value, "serverName")?,
        key,
        optional_str(value, "shortId")?.unwrap_or(""),
    )?;
    with_tls_fingerprint(security, optional_str(value, "fingerprint")?)
}

fn only_one<T>(first: Option<T>, second: Option<T>) -> Result<Option<T>, ClientError> {
    if first.is_some() && second.is_some() {
        return Err(ClientError::InvalidSubscription);
    }
    Ok(first.or(second))
}
