use std::net::{IpAddr, SocketAddr};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::{Map, Value};
use vpn_domain::client::{
    ClientError, NativeProxy, NativeProxyKind, ProxyProfile, ProxyProtocol, ProxyTransport, Secret,
};

use crate::client_normalize::{endpoint_host, parse_json_value};

const CLIENT_FIELDS: &[&str] = &["protocol", "transport", "tcp_settings", "quic_settings"];

pub(crate) fn parse(
    mut value: Value,
    dns_servers: Vec<IpAddr>,
) -> Result<(NativeProxy, ProxyTransport), ClientError> {
    canonicalize_protocol(&mut value)?;
    let (kind, transport) = classify(&value)?;
    if !dns_servers.is_empty()
        && !matches!(
            kind,
            NativeProxyKind::WireGuard | NativeProxyKind::AmneziaWg
        )
    {
        return Err(ClientError::UnsupportedFeature);
    }
    Ok((
        NativeProxy {
            kind,
            settings: Secret::new(value.to_string()),
            dns_servers,
        },
        transport,
    ))
}

fn canonicalize_protocol(value: &mut Value) -> Result<(), ClientError> {
    let Some(protocol) = value.get_mut("protocol").and_then(Value::as_object_mut) else {
        return Ok(());
    };
    if protocol.get("type").and_then(Value::as_str) == Some("amneziawg") {
        if !protocol.get("amnezia").is_some_and(Value::is_object) {
            return Err(ClientError::InvalidSubscription);
        }
        protocol.insert("type".to_owned(), Value::from("wireguard"));
    }
    if let Some(amnezia) = protocol.get_mut("amnezia").and_then(Value::as_object_mut) {
        for field in ["i1", "i2", "i3", "i4", "i5"] {
            if amnezia
                .get(field)
                .and_then(Value::as_str)
                .is_some_and(|value| value.trim().is_empty())
            {
                amnezia.remove(field);
            }
        }
    }
    Ok(())
}

pub(crate) fn set_wireguard_editor_source(profile: &mut ProxyProfile) -> Result<(), ClientError> {
    let ProxyProtocol::Native(native) = &profile.protocol else {
        return Ok(());
    };
    if !matches!(
        native.kind,
        NativeProxyKind::WireGuard | NativeProxyKind::AmneziaWg
    ) {
        return Ok(());
    }
    let address = format!(
        "{}:{}",
        crate::client_normalize::bracket_host(&profile.host),
        profile.port
    );
    let mut settings = to_engine(native, &profile.transport, address)?;
    if native.kind == NativeProxyKind::AmneziaWg {
        settings["protocol"]["type"] = Value::from("amneziawg");
    }
    settings["name"] = Value::from(profile.name.clone());
    settings["dns_servers"] = serde_json::json!(native.dns_servers);
    let text =
        serde_json::to_string_pretty(&settings).map_err(|_| ClientError::InvalidSubscription)?;
    profile.source = Some(Secret::new(text));
    Ok(())
}

pub(crate) fn restore(
    settings: &str,
    dns_servers: Vec<IpAddr>,
) -> Result<NativeProxy, ClientError> {
    let value = parse_json_value(settings).map_err(|_| ClientError::StorageCorrupt)?;
    let (proxy, _) = parse(value, dns_servers).map_err(|_| ClientError::StorageCorrupt)?;
    Ok(proxy)
}

pub(crate) fn to_engine(
    proxy: &NativeProxy,
    transport: &ProxyTransport,
    address: String,
) -> Result<Value, ClientError> {
    let mut value = parse_json_value(proxy.settings.expose())?;
    canonicalize_protocol(&mut value)?;
    let (kind, configured) = classify(&value)?;
    if proxy.kind != kind || transport != &configured {
        return Err(ClientError::InvalidInput);
    }
    value["address"] = Value::from(address);
    Ok(value)
}

fn classify(value: &Value) -> Result<(NativeProxyKind, ProxyTransport), ClientError> {
    let settings = check_fields(value, CLIENT_FIELDS)?;
    let protocol = settings
        .get("protocol")
        .ok_or(ClientError::InvalidSubscription)?;
    let kind = classify_protocol(protocol, 0)?;
    let outer_kind = required_string(protocol, "type")?;
    let default_transport = match outer_kind {
        "wireguard" => "udp",
        "tuic" | "tuicv5" | "hysteria2" | "hy2" => "quic",
        _ => "tcp",
    };
    let configured = match settings.get("transport") {
        Some(value) => value.as_str().ok_or(ClientError::InvalidSubscription)?,
        None => default_transport,
    };
    let transport = match configured {
        "tcp" if default_transport == "tcp" => ProxyTransport::Tcp,
        "udp" if default_transport == "udp" => ProxyTransport::Udp,
        "quic" if default_transport != "udp" => ProxyTransport::Quic {
            certificate_sha256: None,
        },
        _ => return Err(ClientError::UnsupportedFeature),
    };
    check_socket_options(settings, configured)?;
    Ok((kind, transport))
}

fn check_socket_options(settings: &Map<String, Value>, transport: &str) -> Result<(), ClientError> {
    if let Some(options) = settings.get("tcp_settings") {
        if transport != "tcp" {
            return Err(ClientError::UnsupportedFeature);
        }
        check_fields(options, &["no_delay"])?;
    }
    if let Some(options) = settings.get("quic_settings") {
        if transport != "quic" {
            return Err(ClientError::UnsupportedFeature);
        }
        check_quic_options(options)?;
    }
    Ok(())
}

fn check_quic_options(value: &Value) -> Result<(), ClientError> {
    check_fields(
        value,
        &[
            "verify",
            "server_fingerprints",
            "server_fingerprint",
            "sni_hostname",
            "alpn_protocols",
            "alpn_protocol",
            "key_exchange_groups",
        ],
    )?;
    Ok(())
}

fn classify_protocol(value: &Value, depth: u8) -> Result<NativeProxyKind, ClientError> {
    if depth >= 8 {
        return Err(ClientError::UnsupportedFeature);
    }
    let kind = required_string(value, "type")?;
    if let Some(allowed) = wrapper_fields(kind) {
        check_fields(value, allowed)?;
        return classify_protocol(
            value
                .get("protocol")
                .ok_or(ClientError::InvalidSubscription)?,
            depth + 1,
        );
    }
    if kind == "wireguard" {
        if depth != 0 {
            return Err(ClientError::UnsupportedFeature);
        }
        return wireguard(value);
    }
    terminal_protocol(value, kind)
}

fn wrapper_fields(kind: &str) -> Option<&'static [&'static str]> {
    match kind {
        "tls" => Some(&[
            "type",
            "protocol",
            "fingerprint",
            "verify",
            "vision",
            "sni_hostname",
            "alpn_protocols",
            "alpn_protocol",
            "server_fingerprints",
            "server_fingerprint",
            "key_exchange_groups",
            "tls_buffer_size",
        ]),
        "reality" => Some(&[
            "type",
            "protocol",
            "fingerprint",
            "vision",
            "sni_hostname",
            "short_id",
            "public_key",
            "mldsa65_verify",
            "cipher_suites",
            "cipher_suite",
        ]),
        "shadowtls" => Some(&["type", "protocol", "password", "sni_hostname"]),
        "ws" | "websocket" => Some(&[
            "type",
            "protocol",
            "matching_path",
            "matching_headers",
            "max_early_data",
            "early_data_header_name",
            "ping_type",
        ]),
        "xhttp" | "splithttp" => Some(&[
            "type",
            "protocol",
            "path",
            "host",
            "mode",
            "http_version",
            "session_id_placement",
            "session_id_key",
            "seq_placement",
            "seq_key",
            "uplink_http_method",
            "uplink_data_placement",
            "uplink_data_key",
            "uplink_chunk_size",
            "session_id_table",
            "session_id_length",
            "x_padding_bytes",
            "x_padding_obfs_mode",
            "x_padding_key",
            "x_padding_header",
            "x_padding_placement",
            "x_padding_method",
            "sc_max_each_post_bytes",
            "sc_min_posts_interval_ms",
            "headers",
            "no_grpc_header",
            "xmux",
            "download_idle_timeout_secs",
            "packet_pipeline_depth",
            "limits",
            "pool",
            "quic_params",
        ]),
        "sudoku_mask" => Some(&[
            "type",
            "protocol",
            "password",
            "ascii",
            "custom_table",
            "custom_tables",
            "padding_min",
            "padding_max",
            "customTable",
            "customTables",
            "paddingMin",
            "paddingMax",
        ]),
        _ => None,
    }
}

fn terminal_protocol(value: &Value, kind: &str) -> Result<NativeProxyKind, ClientError> {
    use NativeProxyKind::*;
    let (kind, allowed, required): (_, &[_], &[_]) = match kind {
        "socks" | "socks5" => (Socks5, &["type", "username", "password"], &[]),
        "http" => (
            Http,
            &[
                "type",
                "username",
                "password",
                "resolve_hostname",
                "http_version",
            ],
            &[],
        ),
        "vless" => (
            Vless,
            &[
                "type",
                "user_id",
                "vision",
                "encryption",
                "udp_enabled",
                "h2mux",
            ],
            &["user_id"],
        ),
        "vmess" => (
            Vmess,
            &["type", "user_id", "cipher", "udp_enabled", "h2mux"],
            &["user_id", "cipher"],
        ),
        "trojan" => (
            Trojan,
            &["type", "password", "shadowsocks", "h2mux"],
            &["password"],
        ),
        "shadowsocks" | "ss" => (
            Shadowsocks,
            &["type", "cipher", "password", "udp_enabled"],
            &["cipher", "password"],
        ),
        "snell" => (
            Snell,
            &["type", "cipher", "password", "udp_enabled"],
            &["cipher", "password"],
        ),
        "anytls" => (
            AnyTls,
            &[
                "type",
                "password",
                "udp_enabled",
                "padding_scheme",
                "idle_session_check_interval_secs",
                "idle_session_timeout_secs",
                "min_idle_session",
                "disable_reuse",
            ],
            &["password"],
        ),
        "naive" | "naiveproxy" => (
            NaiveProxy,
            &["type", "username", "password", "padding"],
            &["username", "password"],
        ),
        "hysteria2" | "hy2" => (
            Hysteria2,
            &["type", "password", "obfs", "hopping", "udp_enabled"],
            &["password"],
        ),
        "tuic" | "tuicv5" => (
            Tuic,
            &[
                "type",
                "uuid",
                "password",
                "udp_relay_mode",
                "heartbeat_interval_secs",
                "udp_enabled",
            ],
            &["uuid", "password"],
        ),
        "portforward" | "noop" => (PortForward, &["type"], &[]),
        _ => return Err(ClientError::UnsupportedFeature),
    };
    check_fields(value, allowed)?;
    for field in required {
        required_string(value, field)?;
    }
    Ok(kind)
}

fn wireguard(value: &Value) -> Result<NativeProxyKind, ClientError> {
    check_fields(
        value,
        &[
            "type",
            "private_key",
            "public_key",
            "pre_shared_key",
            "addresses",
            "allowed_ips",
            "persistent_keepalive",
            "mtu",
            "resource_limits",
            "amnezia",
        ],
    )?;
    for field in ["private_key", "public_key", "pre_shared_key"] {
        if field == "pre_shared_key" && value.get(field).is_none_or(Value::is_null) {
            continue;
        }
        let key = required_string(value, field)?;
        let mut decoded = zeroize::Zeroizing::new([0_u8; 33]);
        if STANDARD.decode_slice(key, &mut decoded[..]).ok() != Some(32) {
            return Err(ClientError::InvalidSubscription);
        }
    }
    check_cidrs(value, "addresses", true)?;
    check_cidrs(value, "allowed_ips", false)?;
    check_wireguard_mtu(value)?;
    if let Some(amnezia) = value.get("amnezia") {
        if amnezia.get("version").and_then(Value::as_str) != Some("3.1") {
            return Err(ClientError::UnsupportedFeature);
        }
        return Ok(NativeProxyKind::AmneziaWg);
    }
    Ok(NativeProxyKind::WireGuard)
}

fn check_wireguard_mtu(value: &Value) -> Result<(), ClientError> {
    if value
        .get("mtu")
        .is_some_and(|mtu| !mtu.as_u64().is_some_and(|mtu| (1280..=1420).contains(&mtu)))
    {
        return Err(ClientError::InvalidSubscription);
    }
    Ok(())
}

fn check_cidrs(value: &Value, key: &str, required: bool) -> Result<(), ClientError> {
    let Some(value) = value.get(key) else {
        return if required {
            Err(ClientError::InvalidSubscription)
        } else {
            Ok(())
        };
    };
    let items = value
        .as_array()
        .filter(|items| !items.is_empty())
        .ok_or(ClientError::InvalidSubscription)?;
    let mut families = [false; 2];
    for item in items {
        let (address, prefix) = item
            .as_str()
            .and_then(|item| item.split_once('/'))
            .ok_or(ClientError::InvalidSubscription)?;
        let ip: IpAddr = address
            .parse()
            .map_err(|_| ClientError::InvalidSubscription)?;
        if key == "addresses" && std::mem::replace(&mut families[usize::from(ip.is_ipv6())], true) {
            return Err(ClientError::InvalidSubscription);
        }
        let prefix: u8 = prefix
            .parse()
            .map_err(|_| ClientError::InvalidSubscription)?;
        if prefix > if ip.is_ipv4() { 32 } else { 128 } {
            return Err(ClientError::InvalidSubscription);
        }
    }
    Ok(())
}

pub(crate) fn endpoint(value: &str) -> Result<(String, u16), ClientError> {
    let (host, port) = value
        .rsplit_once(':')
        .ok_or(ClientError::InvalidSubscription)?;
    let port = port
        .parse::<u16>()
        .ok()
        .filter(|port| *port != 0)
        .ok_or(ClientError::InvalidSubscription)?;
    Ok((endpoint_host(host)?, port))
}

pub(crate) fn dns(profile: &NativeProxy, chain: &Value) -> Option<Value> {
    (!profile.dns_servers.is_empty()).then(|| {
        let servers = profile
            .dns_servers
            .iter()
            .map(|ip| {
                let endpoint = SocketAddr::new(*ip, 53);
                serde_json::json!({"url": format!("tcp://{endpoint}"), "client_chain": chain})
            })
            .collect::<Vec<_>>();
        serde_json::json!({"servers": servers})
    })
}

fn required_string<'a>(value: &'a Value, key: &str) -> Result<&'a str, ClientError> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or(ClientError::InvalidSubscription)
}

fn check_fields<'a>(
    value: &'a Value,
    allowed: &[&str],
) -> Result<&'a Map<String, Value>, ClientError> {
    let object = value.as_object().ok_or(ClientError::InvalidSubscription)?;
    if object.keys().any(|key| !allowed.contains(&key.as_str())) {
        return Err(ClientError::UnsupportedFeature);
    }
    Ok(object)
}
