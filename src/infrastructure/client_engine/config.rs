use serde_json::{Value, json};
use uuid::{Uuid, Variant, Version};
use vpn_domain::client::{ClientError, ProxyProfile, ProxyProtocol, ProxySecurity, ProxyTransport};
use vpn_domain::client::{WebSocketEarlyData, XhttpMetadata, XhttpPacketOptions, XhttpPlacement};
use vpn_domain::routing::RoutingProfile;

use super::{routing, vless_encryption};

pub(super) fn render(
    profile: &ProxyProfile,
    port: u16,
    profile_routing: Option<&RoutingProfile>,
) -> Result<Vec<u8>, ClientError> {
    let chain = client_chain(profile)?;
    let mut listener = json!({
        "address":format!("127.0.0.1:{port}"),
        "protocol":{"type":"mixed"},
    });
    let document = if let Some(profile_routing) = profile_routing {
        profile_routing.input.validate()?;
        routing::configure_listener(&mut listener, &profile_routing.input);
        vec![routing::proxy_group(chain), listener]
    } else {
        listener["rules"] = json!([forward_rule(&chain)]);
        vec![listener]
    };
    serde_json::to_vec_pretty(&document).map_err(|_| ClientError::EngineInvalid)
}

#[cfg(target_os = "android")]
pub fn render_android_tunnel(
    profile: &ProxyProfile,
    device_fd: i32,
    readiness_port: u16,
) -> Result<String, ClientError> {
    let chain = client_chain(profile)?;
    let document = json!([{
        "device_fd":device_fd,
        "mtu":1500,
        "dns_hijack":true,
        "dns":{"servers":[{"url":"tcp://1.1.1.1", "client_chain":&chain}]},
        "rules":[forward_rule(&chain)]
    }, {
        "address":format!("127.0.0.1:{readiness_port}"),
        "protocol":{"type":"mixed"},
        "rules":"block-all"
    }]);
    serde_json::to_string(&document).map_err(|_| ClientError::EngineInvalid)
}

fn forward_rule(chain: &Value) -> Value {
    json!({"masks":["0.0.0.0/0","::/0"], "action":"allow", "client_chain":chain})
}

fn client_chain(profile: &ProxyProfile) -> Result<Value, ClientError> {
    validate(profile)?;
    let mut protocol = wrap_transport(profile);
    let address = format_host(&profile.host, profile.port);
    if let ProxyTransport::Quic { certificate_sha256 } = &profile.transport {
        return quic_chain(profile, address, protocol, certificate_sha256.as_deref());
    }
    if let ProxyTransport::Xhttp { .. } = &profile.transport
        && let ProxySecurity::Tls { alpn, .. } = &profile.security
        && alpn.as_slice() == ["h3"]
    {
        protocol["http_version"] = json!("h3");
        return quic_chain(profile, address, protocol, None);
    }
    let vision = outer_vision(&profile.protocol);
    protocol = match &profile.security {
        ProxySecurity::None => protocol,
        ProxySecurity::Tls {
            server_name,
            allow_insecure,
            alpn,
            ..
        } => {
            let mut wrapper = json!({
                "type":"tls","sni_hostname":server_name,"verify":!allow_insecure,
                "vision":vision,"protocol":protocol
            });
            if !alpn.is_empty() {
                wrapper["alpn_protocols"] = json!(alpn.as_slice());
            } else if profile.security.fingerprint().is_some()
                && matches!(profile.transport, ProxyTransport::WebSocket { .. })
            {
                wrapper["alpn_protocols"] = json!(["http/1.1"]);
            }
            wrapper
        }
        ProxySecurity::Reality {
            server_name,
            public_key,
            short_id,
            ..
        } => json!({
            "type":"reality","sni_hostname":server_name,"public_key":public_key,
            "short_id":short_id,"vision":vision,"protocol":protocol
        }),
    };
    if let Some(fingerprint) = profile.security.fingerprint() {
        protocol["fingerprint"] = json!(fingerprint.name());
    }
    Ok(json!({"address":address, "protocol":protocol}))
}

fn quic_chain(
    profile: &ProxyProfile,
    address: String,
    protocol: Value,
    fingerprint: Option<&str>,
) -> Result<Value, ClientError> {
    if profile.security.fingerprint().is_some() {
        return Err(ClientError::UnsupportedFeature);
    }
    let ProxySecurity::Tls {
        server_name,
        allow_insecure,
        ..
    } = &profile.security
    else {
        return Err(ClientError::UnsupportedFeature);
    };
    let mut settings = json!({
        "sni_hostname":server_name,"verify":!allow_insecure,"alpn_protocols":["h3"]
    });
    if let Some(fingerprint) = fingerprint {
        settings["server_fingerprints"] = json!([fingerprint]);
    }
    Ok(json!({"address":address,"transport":"quic","quic_settings":settings,"protocol":protocol}))
}

fn wrap_transport(profile: &ProxyProfile) -> Value {
    let protocol = protocol(&profile.protocol);
    match &profile.transport {
        ProxyTransport::Tcp | ProxyTransport::Quic { .. } => protocol,
        ProxyTransport::WebSocket {
            path,
            host,
            early_data,
        } => {
            let mut wrapper = json!({"type":"ws","matching_path":path,"protocol":protocol});
            if let Some(host) = host {
                wrapper["matching_headers"] = json!({"Host":host});
            }
            if let Some(early_data) = early_data {
                wrapper["max_early_data"] = json!(early_data.max_bytes);
                if let Some(header) = &early_data.header_name {
                    wrapper["early_data_header_name"] = json!(header);
                }
            }
            wrapper
        }
        ProxyTransport::Xhttp {
            path,
            host,
            mode,
            metadata,
            packet,
        } => {
            let mut wrapper = json!({
                "type":"xhttp","path":path,"mode":mode.name(),"protocol":protocol,
                "host":host.clone().unwrap_or_else(|| xhttp_authority(profile))
            });
            write_xhttp_metadata(&mut wrapper, metadata);
            write_xhttp_packet(&mut wrapper, packet);
            wrapper
        }
    }
}

fn write_xhttp_metadata(wrapper: &mut Value, metadata: &XhttpMetadata) {
    let fields = [
        (
            "session_id_placement",
            "session_id_key",
            metadata.session_id_placement,
            &metadata.session_id_key,
        ),
        (
            "seq_placement",
            "seq_key",
            metadata.seq_placement,
            &metadata.seq_key,
        ),
    ];
    for (placement_field, key_field, placement, key) in fields {
        if placement != XhttpPlacement::Path {
            wrapper[placement_field] = json!(placement.name());
            if let Some(key) = key {
                wrapper[key_field] = json!(key);
            }
        }
    }
}

fn write_xhttp_packet(wrapper: &mut Value, packet: &XhttpPacketOptions) {
    for (field, range) in [
        ("sc_max_each_post_bytes", packet.max_post_bytes),
        ("sc_min_posts_interval_ms", packet.min_post_interval_ms),
    ] {
        if let Some(range) = range {
            let (min, max) = range.bounds();
            wrapper[field] = if min == max {
                json!(min)
            } else {
                json!(format!("{min}-{max}"))
            };
        }
    }
}

pub(super) fn xhttp_authority(profile: &ProxyProfile) -> String {
    let host = match &profile.security {
        ProxySecurity::Tls { server_name, .. } | ProxySecurity::Reality { server_name, .. } => {
            server_name
        }
        ProxySecurity::None => &profile.host,
    };
    if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]")
    } else {
        host.clone()
    }
}

fn outer_vision(protocol: &ProxyProtocol) -> bool {
    matches!(
        protocol,
        ProxyProtocol::Vless {
            vision: true,
            encryption: None,
            ..
        }
    )
}

pub(crate) fn validate(profile: &ProxyProfile) -> Result<(), ClientError> {
    if profile.host.is_empty()
        || profile.host.len() > 253
        || profile.port == 0
        || profile.host.chars().any(char::is_control)
    {
        return Err(ClientError::InvalidInput);
    }
    validate_protocol(&profile.protocol)?;
    if matches!(profile.protocol, ProxyProtocol::Hysteria2 { .. })
        || matches!(profile.transport, ProxyTransport::Quic { .. })
    {
        return validate_hysteria2_transport(profile);
    }
    if let ProxyTransport::WebSocket {
        path,
        host,
        early_data,
    } = &profile.transport
    {
        validate_websocket(path, host.as_deref(), early_data.as_ref())?;
    }
    if outer_vision(&profile.protocol)
        && (!matches!(profile.transport, ProxyTransport::Tcp)
            || matches!(profile.security, ProxySecurity::None))
    {
        return Err(ClientError::UnsupportedFeature);
    }
    if matches!(profile.security, ProxySecurity::Reality { .. })
        && (!matches!(profile.protocol, ProxyProtocol::Vless { .. })
            || !matches!(
                profile.transport,
                ProxyTransport::Tcp | ProxyTransport::Xhttp { .. }
            ))
    {
        return Err(ClientError::UnsupportedFeature);
    }
    if let ProxyTransport::Xhttp { metadata, .. } = &profile.transport {
        metadata.validate()?;
        validate_xhttp(profile)?;
    }
    Ok(())
}

fn validate_websocket(
    path: &str,
    host: Option<&str>,
    early_data: Option<&WebSocketEarlyData>,
) -> Result<(), ClientError> {
    const MAX_HTTP_LINE: usize = 4096;
    if !path.starts_with('/')
        || path.len() > MAX_HTTP_LINE
        || path.chars().any(char::is_control)
        || host.is_some_and(|host| host.chars().any(char::is_control))
    {
        return Err(ClientError::InvalidInput);
    }
    let Some(early_data) = early_data else {
        return Ok(());
    };
    let host = host.ok_or(ClientError::InvalidInput)?;
    if !path.is_ascii()
        || path.contains([' ', '#'])
        || host.is_empty()
        || host.len() + "Host: ".len() >= MAX_HTTP_LINE
        || reqwest::header::HeaderValue::from_str(host).is_err()
    {
        return Err(ClientError::InvalidInput);
    }
    let request_line = path.len() + "GET  HTTP/1.1".len();
    let carrier_line = if let Some(header) = &early_data.header_name {
        validate_early_data_header(header)?;
        header.len() + ": ".len()
    } else {
        if path.contains('?') {
            return Err(ClientError::UnsupportedFeature);
        }
        request_line
    };
    let encoded = (usize::from(early_data.max_bytes.get()) * 4).div_ceil(3);
    if request_line >= MAX_HTTP_LINE || carrier_line + encoded >= MAX_HTTP_LINE {
        return Err(ClientError::UnsupportedFeature);
    }
    Ok(())
}

fn validate_early_data_header(header: &str) -> Result<(), ClientError> {
    let header = reqwest::header::HeaderName::from_bytes(header.as_bytes())
        .map_err(|_| ClientError::InvalidInput)?;
    match header.as_str() {
        "host"
        | "connection"
        | "upgrade"
        | "content-length"
        | "transfer-encoding"
        | "sec-websocket-key"
        | "sec-websocket-version"
        | "sec-websocket-accept"
        | "sec-websocket-extensions" => Err(ClientError::UnsupportedFeature),
        _ => Ok(()),
    }
}

fn validate_hysteria2_transport(profile: &ProxyProfile) -> Result<(), ClientError> {
    let (
        ProxyProtocol::Hysteria2 { .. },
        ProxyTransport::Quic { certificate_sha256 },
        ProxySecurity::Tls { alpn, .. },
    ) = (&profile.protocol, &profile.transport, &profile.security)
    else {
        return Err(ClientError::UnsupportedFeature);
    };
    if !alpn.is_empty() && alpn.as_slice() != ["h3"] {
        return Err(ClientError::UnsupportedFeature);
    }
    if certificate_sha256
        .as_ref()
        .is_some_and(|pin| pin.len() != 64 || !pin.bytes().all(|byte| byte.is_ascii_hexdigit()))
    {
        return Err(ClientError::InvalidInput);
    }
    Ok(())
}

fn validate_xhttp(profile: &ProxyProfile) -> Result<(), ClientError> {
    if let ProxySecurity::Tls { alpn, .. } = &profile.security
        && !alpn.is_empty()
        && (alpn.as_slice().len() != 1
            || !matches!(alpn.as_slice()[0].as_str(), "http/1.1" | "h2" | "h3"))
    {
        return Err(ClientError::UnsupportedFeature);
    }
    Ok(())
}

fn validate_protocol(protocol: &ProxyProtocol) -> Result<(), ClientError> {
    match protocol {
        ProxyProtocol::Socks5 { username, password }
        | ProxyProtocol::Http { username, password } => {
            if username.is_some() != password.is_some()
                || username
                    .as_ref()
                    .is_some_and(|value| value.expose().len() > 255)
                || password
                    .as_ref()
                    .is_some_and(|value| value.expose().len() > 255)
            {
                return Err(ClientError::UnsupportedFeature);
            }
        }
        ProxyProtocol::Vless {
            user_uuid,
            encryption,
            ..
        } => {
            validate_user_uuid(user_uuid.expose())?;
            if let Some(setting) = encryption {
                vless_encryption::validate(setting.expose())?;
            }
        }
        ProxyProtocol::Vmess { user_uuid, .. } => {
            validate_user_uuid(user_uuid.expose())?;
        }
        ProxyProtocol::Shadowsocks { cipher, .. } => {
            if ![
                "aes-128-gcm",
                "aes-256-gcm",
                "chacha20-ietf-poly1305",
                "2022-blake3-aes-128-gcm",
                "2022-blake3-aes-256-gcm",
                "2022-blake3-chacha20-poly1305",
            ]
            .contains(&cipher.as_str())
            {
                return Err(ClientError::UnsupportedFeature);
            }
        }
        ProxyProtocol::Trojan { password } => {
            if password.expose().is_empty() {
                return Err(ClientError::InvalidInput);
            }
        }
        ProxyProtocol::Hysteria2 {
            password,
            salamander_password,
            ..
        } => {
            validate_hysteria2_credentials(
                password.expose(),
                salamander_password.as_ref().map(|value| value.expose()),
            )?;
        }
    }
    if let ProxyProtocol::Vmess { cipher, .. } = protocol
        && !["any", "aes-128-gcm", "chacha20-poly1305", "none"].contains(&cipher.as_str())
    {
        return Err(ClientError::UnsupportedFeature);
    }
    Ok(())
}

fn validate_hysteria2_credentials(
    password: &str,
    salamander: Option<&str>,
) -> Result<(), ClientError> {
    if password.is_empty()
        || reqwest::header::HeaderValue::from_str(password).is_err()
        || salamander.is_some_and(|value| value.len() < 4)
    {
        return Err(ClientError::InvalidInput);
    }
    Ok(())
}

fn validate_user_uuid(value: &str) -> Result<(), ClientError> {
    let uuid = Uuid::parse_str(value).map_err(|_| ClientError::InvalidInput)?;
    if uuid.get_version() != Some(Version::Random) || uuid.get_variant() != Variant::RFC4122 {
        return Err(ClientError::UnsupportedFeature);
    }
    Ok(())
}

fn protocol(protocol: &ProxyProtocol) -> Value {
    match protocol {
        ProxyProtocol::Socks5 { username, password } => auth("socks", username, password),
        ProxyProtocol::Http { username, password } => auth("http", username, password),
        ProxyProtocol::Vless {
            user_uuid,
            vision,
            encryption,
        } => {
            let mut value = json!({"type":"vless","user_id":user_uuid.expose()});
            if let Some(setting) = encryption {
                value["encryption"] = json!(setting.expose());
                value["vision"] = json!(vision);
            }
            value
        }
        ProxyProtocol::Vmess { user_uuid, cipher } => {
            json!({"type":"vmess","user_id":user_uuid.expose(),"cipher":cipher})
        }
        ProxyProtocol::Trojan { password } => json!({"type":"trojan","password":password.expose()}),
        ProxyProtocol::Hysteria2 {
            password,
            salamander_password,
            udp_enabled,
        } => {
            let mut value =
                json!({"type":"hysteria2","password":password.expose(),"udp_enabled":udp_enabled});
            if let Some(password) = salamander_password {
                value["obfs"] = json!({"type":"salamander","password":password.expose()});
            }
            value
        }
        ProxyProtocol::Shadowsocks { cipher, password } => {
            json!({"type":"shadowsocks","cipher":cipher,"password":password.expose()})
        }
    }
}

fn auth(
    kind: &str,
    username: &Option<vpn_domain::client::Secret>,
    password: &Option<vpn_domain::client::Secret>,
) -> Value {
    let mut value = json!({"type":kind});
    if let Some(username) = username {
        value["username"] = Value::String(username.expose().to_owned());
    }
    if let Some(password) = password {
        value["password"] = Value::String(password.expose().to_owned());
    }
    value
}

fn format_host(host: &str, port: u16) -> String {
    if host.starts_with('[') && host.ends_with(']') {
        format!("{host}:{port}")
    } else if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client_normalize::SubscriptionNormalizer;
    use vpn_application::client::ClientNormalizerPort;
    use vpn_domain::routing::{DnsPolicy, RouteAction, RouteRule, RouteTarget, RoutingInput};

    #[test]
    fn routing_serializes_shared_upstream_credentials_once() {
        let profile = SubscriptionNormalizer
            .normalize(b"trojan://routing-fixture-credential@127.0.0.1:443#Probe")
            .unwrap()
            .remove(0);
        let policy = RoutingProfile {
            uuid: "fixture".into(),
            revision: 1,
            input: RoutingInput {
                title: "Shared outbound".into(),
                rules: ["one.example", "two.example"]
                    .into_iter()
                    .map(|name| RouteRule {
                        target: RouteTarget::Domain(name.into()),
                        action: RouteAction::Proxy,
                    })
                    .collect(),
                dns: DnsPolicy::Automatic,
            },
        };
        let rendered = String::from_utf8(render(&profile, 1080, Some(&policy)).unwrap()).unwrap();
        assert_eq!(rendered.matches("routing-fixture-credential").count(), 1);
    }

    #[test]
    fn websocket_early_data_validates_the_selected_carrier() {
        let settings = [
            ("/ws", 2560, "Sec-WebSocket-Protocol", Ok(())),
            ("/ws", 2560, "", Ok(())),
            ("/ws?token=value", 2560, "X-Start", Ok(())),
            (
                "/ws?token=value",
                2560,
                "",
                Err(ClientError::UnsupportedFeature),
            ),
            ("/ws", 8192, "X-Start", Err(ClientError::UnsupportedFeature)),
            ("/ws", 2048, "Host", Err(ClientError::UnsupportedFeature)),
            ("/ws", 2048, "bad\r\nname", Err(ClientError::InvalidInput)),
        ];
        for (path, max, header, expected) in settings {
            let input = json!({
                "type":"trojan","server":"localhost","server_port":443,"password":"fixture",
                "transport":{"type":"ws","path":path,"max_early_data":max,"early_data_header_name":header}
            });
            let profile = SubscriptionNormalizer
                .normalize(input.to_string().as_bytes())
                .unwrap()
                .remove(0);
            assert_eq!(validate(&profile), expected, "{path}, {max}, {header:?}");
        }
    }

    #[test]
    fn xhttp_renders_nondefault_metadata_as_blazecore_options() {
        let input = json!({
            "type":"vless","server":"localhost","server_port":7443,
            "uuid":"fa9c41b7-fc31-4ab0-a275-1c67aa9aab31",
            "transport":{"type":"xhttp","mode":"packet-up","path":"/api",
                "session_placement":"cookie","seq_placement":"header","seq_key":"X-Packet",
                "sc_max_each_post_bytes":"128000-256000","sc_min_posts_interval_ms":5,
                "uplink_http_method":"POST"}
        });
        let profile = SubscriptionNormalizer
            .normalize(input.to_string().as_bytes())
            .unwrap()
            .remove(0);
        let protocol = &client_chain(&profile).unwrap()["protocol"];
        assert_eq!(protocol["session_id_placement"], "cookie");
        assert!(protocol.get("session_id_key").is_none());
        assert_eq!(protocol["seq_placement"], "header");
        assert_eq!(protocol["seq_key"], "X-Packet");
        assert_eq!(protocol["path"], "/api");
        assert_eq!(protocol["sc_max_each_post_bytes"], "128000-256000");
        assert_eq!(protocol["sc_min_posts_interval_ms"], 5);
        assert!(protocol.get("uplink_http_method").is_none());
        assert!(protocol.get("seqPlacement").is_none());
    }

    #[test]
    fn hysteria2_uses_quic_with_tls_validation_and_tcp_only_forwarding() {
        let input = json!({
            "type":"hysteria2","server":"localhost","server_port":7443,"password":"fixture",
            "network":"tcp","tls":{"enabled":true,"server_name":"edge.example.test"},
            "obfs":{"type":"salamander","password":"fixture-obfs"}
        });
        let profiles = SubscriptionNormalizer
            .normalize(input.to_string().as_bytes())
            .unwrap();
        let chain = client_chain(&profiles[0]).unwrap();
        assert_eq!(
            chain,
            json!({
                "address":"localhost:7443","transport":"quic",
                "quic_settings":{"sni_hostname":"edge.example.test","verify":true,"alpn_protocols":["h3"]},
                "protocol":{"type":"hysteria2","password":"fixture","udp_enabled":false,
                    "obfs":{"type":"salamander","password":"fixture-obfs"}}
            })
        );
        let mut bad = profiles[0].clone();
        bad.security = ProxySecurity::None;
        assert_eq!(client_chain(&bad), Err(ClientError::UnsupportedFeature));
    }

    #[test]
    fn vless_and_vmess_reject_uuid_forms_the_pinned_engine_cannot_use() {
        for protocol in ["vless", "vmess"] {
            for (id, expected) in [
                ("a5c41b77-fc31-4ab0-a275-1c67aa9aab31", Ok(())),
                (
                    "feb54431-301b-52bb-a6dd-e1e93e81bb9e",
                    Err(ClientError::UnsupportedFeature),
                ),
                (
                    "a5c41b77-fc31-4ab0-c275-1c67aa9aab31",
                    Err(ClientError::UnsupportedFeature),
                ),
            ] {
                let document = json!({
                    "protocol":protocol,
                    "settings":{"id":id,"address":"localhost","port":7443}
                });
                let profiles = SubscriptionNormalizer
                    .normalize(document.to_string().as_bytes())
                    .unwrap();
                assert_eq!(validate(&profiles[0]), expected, "{protocol}");
            }
        }
    }
}
