use std::num::NonZeroU16;

use serde_json::Value;

use super::json::{
    check_empty, fields, flag, host_header, optional_str, port, string, tls_alpn, zero,
};
use super::{
    AlpnProtocols, ClientError, ProxyProfile, ProxyProtocol, ProxySecurity, ProxyTransport,
    WebSocketEarlyData, bracket_host, endpoint_host, make_profile, proxy_auth, reality,
    shadowsocks, trojan, verified_tls, vless, vmess, websocket,
};

pub(super) fn parse_outbound(value: &Value) -> Result<ProxyProfile, ClientError> {
    let kind = string(value, "type")?;
    if kind == "hysteria2" {
        return super::hysteria2::parse_sing_box(value);
    }
    let protocol = parse_protocol(value, kind)?;
    if let Some(mux) = value.get("multiplex") {
        default_disabled(mux)?;
    }
    check_empty(value, "network", |option| option.as_str() == Some(""))?;
    let host = string(value, "server")?;
    let server_port = port(value, "server_port")?;
    let transport = match value.get("transport") {
        Some(value) => parse_transport(value, host, server_port)?,
        None => ProxyTransport::Tcp,
    };
    let security = match value.get("tls") {
        Some(value) => parse_tls(value, host)?,
        None => ProxySecurity::None,
    };
    make_profile(
        optional_str(value, "tag")?.unwrap_or(""),
        host,
        server_port,
        protocol,
        transport,
        security,
    )
}

fn parse_protocol(value: &Value, kind: &str) -> Result<ProxyProtocol, ClientError> {
    match kind {
        "socks" => {
            fields(
                value,
                &[
                    "type",
                    "tag",
                    "server",
                    "server_port",
                    "version",
                    "username",
                    "password",
                    "network",
                    "udp_over_tcp",
                ],
            )?;
            check_empty(value, "version", |option| option.as_str() == Some("5"))?;
            check_empty(value, "udp_over_tcp", |option| {
                option.as_bool() == Some(false)
            })?;
            parse_auth(value, kind)
        }
        "http" => {
            fields(
                value,
                &[
                    "type",
                    "tag",
                    "server",
                    "server_port",
                    "username",
                    "password",
                    "tls",
                    "version",
                    "path",
                    "headers",
                    "disable_version_fallback",
                ],
            )?;
            check_empty(value, "version", |option| option.as_u64() == Some(1))?;
            check_empty(value, "path", |option| option.as_str() == Some(""))?;
            check_empty(value, "headers", |option| {
                option.as_object().is_some_and(serde_json::Map::is_empty)
            })?;
            check_empty(value, "disable_version_fallback", |option| {
                option.as_bool() == Some(false)
            })?;
            parse_auth(value, kind)
        }
        "vless" => {
            fields(
                value,
                &[
                    "type",
                    "tag",
                    "server",
                    "server_port",
                    "uuid",
                    "flow",
                    "network",
                    "tls",
                    "transport",
                    "multiplex",
                    "packet_encoding",
                ],
            )?;
            check_empty(value, "packet_encoding", |option| {
                option.as_str() == Some("xudp")
            })?;
            vless(
                string(value, "uuid")?,
                optional_str(value, "flow")?.unwrap_or(""),
                "none",
            )
        }
        "vmess" => {
            fields(
                value,
                &[
                    "type",
                    "tag",
                    "server",
                    "server_port",
                    "uuid",
                    "security",
                    "alter_id",
                    "global_padding",
                    "authenticated_length",
                    "network",
                    "tls",
                    "transport",
                    "multiplex",
                    "packet_encoding",
                ],
            )?;
            check_empty(value, "alter_id", zero)?;
            check_empty(value, "global_padding", |option| {
                option.as_bool() == Some(false)
            })?;
            check_empty(value, "authenticated_length", |option| {
                option.as_bool() == Some(false)
            })?;
            check_empty(value, "packet_encoding", |option| {
                option.as_str() == Some("")
            })?;
            vmess(
                string(value, "uuid")?,
                optional_str(value, "security")?.unwrap_or("auto"),
            )
        }
        "trojan" => {
            fields(
                value,
                &[
                    "type",
                    "tag",
                    "server",
                    "server_port",
                    "password",
                    "network",
                    "tls",
                    "transport",
                    "multiplex",
                ],
            )?;
            trojan(string(value, "password")?)
        }
        "shadowsocks" => {
            fields(
                value,
                &[
                    "type",
                    "tag",
                    "server",
                    "server_port",
                    "method",
                    "password",
                    "plugin",
                    "plugin_opts",
                    "network",
                    "udp_over_tcp",
                    "multiplex",
                ],
            )?;
            for field in ["plugin", "plugin_opts"] {
                check_empty(value, field, |option| option.as_str() == Some(""))?;
            }
            check_empty(value, "udp_over_tcp", |option| {
                option.as_bool() == Some(false)
            })?;
            shadowsocks(string(value, "method")?, string(value, "password")?)
        }
        _ => Err(ClientError::UnsupportedFeature),
    }
}

fn parse_auth(value: &Value, kind: &str) -> Result<ProxyProtocol, ClientError> {
    proxy_auth(
        kind,
        optional_str(value, "username")?,
        optional_str(value, "password")?,
    )
}

fn default_disabled(value: &Value) -> Result<(), ClientError> {
    fields(value, &["enabled"])?;
    check_empty(value, "enabled", |option| option.as_bool() == Some(false))
}

fn parse_transport(value: &Value, server: &str, port: u16) -> Result<ProxyTransport, ClientError> {
    if string(value, "type")? != "ws" {
        return Err(ClientError::UnsupportedFeature);
    }
    fields(
        value,
        &[
            "type",
            "path",
            "headers",
            "max_early_data",
            "early_data_header_name",
        ],
    )?;
    let path = optional_str(value, "path")?
        .filter(|path| !path.is_empty())
        .unwrap_or("/");
    let host = host_header(value)?.filter(|host| !host.is_empty());
    let early_data = parse_early_data(value)?;
    let default_host = if host.is_none() && early_data.is_some() {
        Some(format!("{}:{port}", bracket_host(&endpoint_host(server)?)))
    } else {
        None
    };
    websocket(path, host.or(default_host.as_deref()), early_data)
}

fn parse_early_data(value: &Value) -> Result<Option<WebSocketEarlyData>, ClientError> {
    let max = match value.get("max_early_data") {
        None => 0,
        Some(max) if zero(max) => 0,
        Some(max) => max
            .as_u64()
            .and_then(|max| u16::try_from(max).ok())
            .ok_or(ClientError::InvalidSubscription)?,
    };
    let header = optional_str(value, "early_data_header_name")?.filter(|name| !name.is_empty());
    match (NonZeroU16::new(max), header) {
        (None, Some(_)) => Err(ClientError::InvalidSubscription),
        (None, None) => Ok(None),
        (Some(max_bytes), header) => Ok(Some(WebSocketEarlyData {
            max_bytes,
            header_name: header.map(str::to_ascii_lowercase),
        })),
    }
}

pub(super) fn parse_tls(value: &Value, host: &str) -> Result<ProxySecurity, ClientError> {
    if !flag(value, "enabled")? {
        default_disabled(value)?;
        return Ok(ProxySecurity::None);
    }
    fields(
        value,
        &[
            "enabled",
            "server_name",
            "insecure",
            "disable_sni",
            "utls",
            "reality",
            "alpn",
        ],
    )?;
    check_empty(value, "disable_sni", |option| {
        option.as_bool() == Some(false)
    })?;
    if let Some(utls) = value.get("utls") {
        default_disabled(utls)?;
    }
    let name = optional_str(value, "server_name")?
        .filter(|name| !name.is_empty())
        .unwrap_or(host);
    let insecure = flag(value, "insecure")?;
    let alpn = tls_alpn(value)?;
    if let Some(value) = value.get("reality") {
        return parse_reality(value, name, insecure, alpn);
    }
    verified_tls(name, insecure, alpn)
}

fn parse_reality(
    value: &Value,
    name: &str,
    insecure: bool,
    alpn: AlpnProtocols,
) -> Result<ProxySecurity, ClientError> {
    if !flag(value, "enabled")? {
        default_disabled(value)?;
        return verified_tls(name, insecure, alpn);
    }
    fields(value, &["enabled", "public_key", "short_id"])?;
    if insecure || !alpn.is_empty() {
        return Err(ClientError::UnsupportedFeature);
    }
    reality(
        name,
        string(value, "public_key")?,
        optional_str(value, "short_id")?.unwrap_or(""),
    )
}
