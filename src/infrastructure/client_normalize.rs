use std::net::IpAddr;
use std::num::NonZeroU16;

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD};
use url::Host;
use vpn_application::MAX_SUBSCRIPTION_BYTES;
use vpn_application::client::{ClientError, ClientNormalizerPort, MAX_PROFILES};
use vpn_domain::client::{
    AlpnProtocols, ProxyProfile, ProxyProtocol, ProxySecurity, ProxyTransport, Secret,
    TlsFingerprint, WebSocketEarlyData,
};

mod amnezia;
mod blazecore;
mod hysteria2;
mod json;
mod links;
mod sing_box;
mod subscription;
mod wireguard;
mod xhttp;
mod xray;
mod xray_graph;

pub(crate) use json::parse_value as parse_json_value;

#[cfg(test)]
mod tests;

pub struct SubscriptionNormalizer;

impl ClientNormalizerPort for SubscriptionNormalizer {
    fn normalize(&self, input: &[u8]) -> Result<Vec<ProxyProfile>, ClientError> {
        let mut profiles = parse_input(input, parse_document)?;
        prepare_editor_sources(&mut profiles)?;
        Ok(profiles)
    }

    fn normalize_subscription(
        &self,
        input: &[u8],
    ) -> Result<vpn_application::client::NormalizedSubscription, ClientError> {
        let mut result = subscription::parse(input)?;
        prepare_editor_sources(&mut result.profiles)?;
        Ok(result)
    }
}

fn prepare_editor_sources(profiles: &mut [ProxyProfile]) -> Result<(), ClientError> {
    profiles
        .iter_mut()
        .try_for_each(crate::native_profile::set_wireguard_editor_source)
}

fn parse_input<T>(
    input: &[u8],
    parse_document: impl FnOnce(&str) -> Result<T, ClientError>,
) -> Result<T, ClientError> {
    let text = document_text(input)?;
    if text.starts_with(['{', '['])
        || text.contains("://")
        || wireguard::recognizes(text)
        || json::recognizes_native_yaml(text)
    {
        return parse_document(text);
    }
    parse_document(document_text(&decode_base64(text)?)?)
}

fn document_text(input: &[u8]) -> Result<&str, ClientError> {
    if input.len() > MAX_SUBSCRIPTION_BYTES {
        return Err(ClientError::PayloadTooLarge);
    }
    let text = std::str::from_utf8(input).map_err(|_| ClientError::InvalidSubscription)?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(text).trim();
    if text.is_empty() {
        return Err(ClientError::InvalidSubscription);
    }
    Ok(text)
}

fn parse_document(text: &str) -> Result<Vec<ProxyProfile>, ClientError> {
    if wireguard::recognizes(text) {
        return wireguard::parse(text).map(|profile| vec![profile]);
    }
    if text.starts_with(['{', '[']) {
        return json::parse_document(text);
    }
    if json::recognizes_native_yaml(text) {
        return json::parse_native_yaml(text);
    }
    collect_profiles(
        text.lines().map(str::trim).filter(|line| !line.is_empty()),
        links::parse_share_link,
    )
}

fn collect_profiles<T>(
    inputs: impl IntoIterator<Item = T>,
    parse: impl Fn(T) -> Result<ProxyProfile, ClientError>,
) -> Result<Vec<ProxyProfile>, ClientError> {
    let mut profiles = Vec::new();
    for value in inputs {
        if profiles.len() == MAX_PROFILES {
            return Err(ClientError::PayloadTooLarge);
        }
        profiles.push(parse(value)?);
    }
    if profiles.is_empty() {
        return Err(ClientError::InvalidSubscription);
    }
    Ok(profiles)
}

fn decode_base64(text: &str) -> Result<Vec<u8>, ClientError> {
    let compact: Vec<u8> = text
        .bytes()
        .filter(|byte| !byte.is_ascii_whitespace())
        .collect();
    let engines = [&STANDARD, &STANDARD_NO_PAD, &URL_SAFE, &URL_SAFE_NO_PAD];
    engines
        .iter()
        .find_map(|engine| engine.decode(&compact).ok())
        .ok_or(ClientError::InvalidSubscription)
}

pub(crate) fn endpoint_host(host: &str) -> Result<String, ClientError> {
    if host.is_empty() || host.len() > 253 {
        return Err(ClientError::InvalidSubscription);
    }
    let bare = host
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .unwrap_or(host);
    if let Ok(ip) = bare.parse::<IpAddr>() {
        return Ok(ip.to_string());
    }
    match Host::parse(host) {
        Ok(Host::Domain(domain)) if !domain.is_empty() => Ok(domain),
        _ => Err(ClientError::InvalidSubscription),
    }
}

fn make_profile(
    name: &str,
    host: &str,
    port: u16,
    protocol: ProxyProtocol,
    mut transport: ProxyTransport,
    security: ProxySecurity,
) -> Result<ProxyProfile, ClientError> {
    if name.len() > 256 || name.chars().any(char::is_control) {
        return Err(ClientError::InvalidSubscription);
    }
    let host = endpoint_host(host)?;
    if matches!(transport, ProxyTransport::Quic { .. }) && security.fingerprint().is_some() {
        return Err(ClientError::UnsupportedFeature);
    }
    if let ProxyTransport::WebSocket {
        host: authority,
        early_data: Some(_),
        ..
    } = &mut transport
        && authority.is_none()
    {
        let ws_host = match &security {
            ProxySecurity::Tls { server_name, .. } | ProxySecurity::Reality { server_name, .. } => {
                server_name
            }
            ProxySecurity::None => &host,
        };
        *authority = Some(bracket_host(ws_host));
    }
    let name = if name.is_empty() {
        format!("{} {}", protocol.name(), host)
    } else {
        name.to_owned()
    };
    Ok(ProxyProfile {
        profile_uuid: String::new(),
        name,
        source: None,
        host,
        port,
        protocol,
        transport,
        security,
    })
}

fn proxy_auth(
    kind: &str,
    username: Option<&str>,
    password: Option<&str>,
) -> Result<ProxyProtocol, ClientError> {
    let http = matches!(kind, "http" | "https");
    let (username, password) = match (username, password) {
        (None | Some(""), None | Some("")) => (None, None),
        (Some(user), Some(password)) if !user.is_empty() && (http || !password.is_empty()) => {
            if http
                && (user.contains(':')
                    || [user, password]
                        .iter()
                        .any(|text| text.bytes().any(|value| value.is_ascii_control())))
            {
                return Err(ClientError::InvalidSubscription);
            }
            (Some(Secret::new(user)), Some(Secret::new(password)))
        }
        _ => return Err(ClientError::InvalidSubscription),
    };
    Ok(if http {
        ProxyProtocol::Http { username, password }
    } else {
        ProxyProtocol::Socks5 { username, password }
    })
}

fn vless(id: &str, flow: &str, encryption: &str) -> Result<ProxyProtocol, ClientError> {
    if !matches!(flow, "" | "xtls-rprx-vision") {
        return Err(ClientError::UnsupportedFeature);
    }
    Ok(ProxyProtocol::Vless {
        user_uuid: user_uuid(id)?,
        vision: !flow.is_empty(),
        encryption: (!matches!(encryption, "" | "none")).then(|| Secret::new(encryption)),
    })
}

fn vmess(id: &str, cipher: &str) -> Result<ProxyProtocol, ClientError> {
    let cipher = match cipher {
        "" | "auto" => "any",
        "chacha20-ietf-poly1305" => "chacha20-poly1305",
        "any" | "aes-128-gcm" | "chacha20-poly1305" | "none" => cipher,
        _ => return Err(ClientError::UnsupportedFeature),
    };
    Ok(ProxyProtocol::Vmess {
        user_uuid: user_uuid(id)?,
        cipher: cipher.to_owned(),
    })
}

fn user_uuid(id: &str) -> Result<Secret, ClientError> {
    uuid::Uuid::parse_str(id)
        .map(|value| Secret::new(value.to_string()))
        .map_err(|_| ClientError::InvalidSubscription)
}

fn trojan(password: &str) -> Result<ProxyProtocol, ClientError> {
    if password.is_empty() {
        return Err(ClientError::InvalidSubscription);
    }
    Ok(ProxyProtocol::Trojan {
        password: Secret::new(password),
    })
}

fn shadowsocks(cipher: &str, password: &str) -> Result<ProxyProtocol, ClientError> {
    if password.is_empty() {
        return Err(ClientError::InvalidSubscription);
    }
    let cipher = match cipher {
        "chacha20-poly1305" => "chacha20-ietf-poly1305",
        "aes-128-gcm"
        | "aes-256-gcm"
        | "chacha20-ietf-poly1305"
        | "2022-blake3-aes-128-gcm"
        | "2022-blake3-aes-256-gcm"
        | "2022-blake3-chacha20-poly1305" => cipher,
        _ => return Err(ClientError::UnsupportedFeature),
    };
    if let Some(key_bytes) = match cipher {
        "2022-blake3-aes-128-gcm" => Some(16),
        "2022-blake3-chacha20-poly1305" if password.contains(':') => {
            return Err(ClientError::UnsupportedFeature);
        }
        "2022-blake3-aes-256-gcm" | "2022-blake3-chacha20-poly1305" => Some(32),
        _ => None,
    } {
        for encoded in password.split(':') {
            if STANDARD
                .decode(encoded)
                .map_or(true, |key| key.len() != key_bytes)
            {
                return Err(ClientError::InvalidSubscription);
            }
        }
    }
    Ok(ProxyProtocol::Shadowsocks {
        cipher: cipher.to_owned(),
        password: Secret::new(password),
    })
}

fn verified_tls(
    server_name: &str,
    insecure: bool,
    alpn: AlpnProtocols,
) -> Result<ProxySecurity, ClientError> {
    Ok(ProxySecurity::Tls {
        server_name: endpoint_host(server_name)?,
        allow_insecure: insecure,
        alpn,
        fingerprint: None,
    })
}

fn reality(server_name: &str, key: &str, short_id: &str) -> Result<ProxySecurity, ClientError> {
    let key = decode_base64(key)?;
    if key.len() != 32
        || short_id.len() > 16
        || !short_id.len().is_multiple_of(2)
        || !short_id.bytes().all(|value| value.is_ascii_hexdigit())
    {
        return Err(ClientError::InvalidSubscription);
    }
    Ok(ProxySecurity::Reality {
        server_name: endpoint_host(server_name)?,
        public_key: URL_SAFE_NO_PAD.encode(key),
        short_id: short_id.to_ascii_lowercase(),
        fingerprint: None,
    })
}

fn with_tls_fingerprint(
    mut security: ProxySecurity,
    value: Option<&str>,
) -> Result<ProxySecurity, ClientError> {
    let Some(value) = value.filter(|value| !value.is_empty()) else {
        return Ok(security);
    };
    let fingerprint = Some(TlsFingerprint::parse(value)?);
    match &mut security {
        ProxySecurity::Tls {
            fingerprint: selected,
            ..
        }
        | ProxySecurity::Reality {
            fingerprint: selected,
            ..
        } => *selected = fingerprint,
        ProxySecurity::None => return Err(ClientError::UnsupportedFeature),
    }
    Ok(security)
}

fn websocket(
    path: &str,
    host: Option<&str>,
    early_data: Option<WebSocketEarlyData>,
) -> Result<ProxyTransport, ClientError> {
    if !path.starts_with('/')
        || path.len() > 4096
        || path.contains('#')
        || path.chars().any(|ch| ch.is_whitespace() || ch.is_control())
    {
        return Err(ClientError::InvalidSubscription);
    }
    Ok(ProxyTransport::WebSocket {
        path: path.to_owned(),
        host: http_authority(host)?,
        early_data,
    })
}

pub(crate) fn bracket_host(host: &str) -> String {
    if host.parse::<std::net::Ipv6Addr>().is_ok() {
        format!("[{host}]")
    } else {
        host.to_owned()
    }
}

fn xray_websocket(path: &str, host: Option<&str>) -> Result<ProxyTransport, ClientError> {
    let Some((prefix, query)) = path.split_once('?') else {
        return websocket(path, host, None);
    };
    if path.len() > 4096 || path.contains('#') {
        return Err(ClientError::InvalidSubscription);
    }
    let mut max = None;
    let mut forwarded = Vec::new();
    for part in query.split('&') {
        match url::form_urlencoded::parse(part.as_bytes()).next() {
            Some((key, value)) if key == "ed" => {
                let value = value
                    .parse::<u16>()
                    .map_err(|_| ClientError::InvalidSubscription)?;
                if max.replace(value).is_some() {
                    return Err(ClientError::InvalidSubscription);
                }
            }
            _ => forwarded.push(part),
        }
    }
    let Some(max) = max else {
        return websocket(path, host, None);
    };
    let path = if forwarded.is_empty() {
        prefix.to_owned()
    } else {
        format!("{prefix}?{}", forwarded.join("&"))
    };
    let early_data = NonZeroU16::new(max).map(|max_bytes| WebSocketEarlyData {
        max_bytes,
        header_name: Some("sec-websocket-protocol".into()),
    });
    websocket(&path, host, early_data)
}

fn http_authority(host: Option<&str>) -> Result<Option<String>, ClientError> {
    let host = host.filter(|value| !value.is_empty());
    if let Some(host) = host {
        let authority = url::Url::parse(&format!("http://{host}"))
            .map_err(|_| ClientError::InvalidSubscription)?;
        if authority.host_str().is_none()
            || host.contains(['/', '\\', '?', '#', '@', '%'])
            || host
                .chars()
                .any(|value| value.is_whitespace() || value.is_control())
        {
            return Err(ClientError::InvalidSubscription);
        }
    }
    Ok(host.map(str::to_owned))
}
