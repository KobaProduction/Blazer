use std::collections::HashMap;

use percent_encoding::{NON_ALPHANUMERIC, percent_decode_str, utf8_percent_encode};
use serde_json::Value;
use url::Url;
use vpn_domain::client::TcpProbeEndpoint;

use super::json::{check_empty, optional_str, parse_value, port, string, zero};
use super::{
    AlpnProtocols, ClientError, ProxyProfile, ProxyProtocol, ProxySecurity, ProxyTransport,
    decode_base64, make_profile, proxy_auth, reality, shadowsocks, trojan, verified_tls, vless,
    vmess, with_tls_fingerprint, xhttp, xray_websocket,
};

pub(super) fn parse_share_link(text: &str) -> Result<ProxyProfile, ClientError> {
    if has_unescaped_space_or_control(text) {
        return Err(ClientError::InvalidSubscription);
    }
    let (scheme, body) = text
        .split_once("://")
        .ok_or(ClientError::InvalidSubscription)?;
    if scheme.eq_ignore_ascii_case("vmess") && !body.contains('@') {
        return parse_v2rayn(body);
    }
    if scheme.eq_ignore_ascii_case("ss") {
        return parse_shadowsocks(body);
    }
    if is_socks_scheme(scheme) {
        return parse_socks_link(text, scheme, body);
    }
    parse_url(text)
}

pub(super) fn tcp_probe_endpoint(text: &str) -> Option<TcpProbeEndpoint> {
    if has_unescaped_space_or_control(text) {
        return None;
    }
    let url = Url::parse(text).ok()?;
    let default_port = match url.scheme() {
        "vless" | "vmess" | "socks" | "socks5" => None,
        "trojan" => Some(443),
        "http" | "https" => url.port_or_known_default(),
        _ => return None,
    };
    let mut transports = url.query_pairs().filter(|(name, _)| name == "type");
    if let Some((_, transport)) = transports.next()
        && (!super::subscription::is_tcp_transport(&transport) || transports.next().is_some())
    {
        return None;
    }
    let host = super::endpoint_host(url.host_str()?).ok()?;
    TcpProbeEndpoint::new(&host, url.port().or(default_port)?)
}

fn has_unescaped_space_or_control(text: &str) -> bool {
    text.chars()
        .any(|value| value.is_whitespace() || value.is_control())
}

fn is_socks_scheme(scheme: &str) -> bool {
    scheme.eq_ignore_ascii_case("socks") || scheme.eq_ignore_ascii_case("socks5")
}

fn parse_socks_link(text: &str, scheme: &str, body: &str) -> Result<ProxyProfile, ClientError> {
    let (authority, suffix) = body.split_at(body.find(['?', '#']).unwrap_or(body.len()));
    if let Some((userinfo, endpoint)) = authority.rsplit_once('@') {
        if userinfo.is_empty() || userinfo.contains(':') {
            return parse_url(text);
        }
        let credentials = decode_base64_text(&decode_component(userinfo)?)?;
        return parse_socks_userinfo(scheme, &credentials, endpoint, suffix);
    }
    let Ok(decoded) = decode_base64_text(authority) else {
        return parse_url(text);
    };
    if let Some((inner_scheme, _)) = decoded.split_once("://").filter(|(scheme, _)| {
        scheme.starts_with(|c: char| c.is_ascii_alphabetic())
            && scheme
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"+.-".contains(&byte))
    }) {
        if is_socks_scheme(inner_scheme) {
            return parse_encoded_socks_url(&decoded, suffix);
        }
        return Err(ClientError::UnsupportedFormat);
    }
    match decoded.rsplit_once('@') {
        Some((credentials, endpoint)) => {
            parse_socks_userinfo(scheme, credentials, endpoint, suffix)
        }
        None => parse_url(text),
    }
}

fn parse_encoded_socks_url(decoded: &str, suffix: &str) -> Result<ProxyProfile, ClientError> {
    if has_unescaped_space_or_control(decoded)
        || decoded.contains('#') && !suffix.is_empty()
        || decoded.contains('?') && suffix.starts_with('?')
    {
        return Err(ClientError::InvalidSubscription);
    }
    parse_url(&format!("{decoded}{suffix}"))
}

fn parse_socks_userinfo(
    scheme: &str,
    credentials: &str,
    endpoint: &str,
    suffix: &str,
) -> Result<ProxyProfile, ClientError> {
    if has_unescaped_space_or_control(endpoint) {
        return Err(ClientError::InvalidSubscription);
    }
    if endpoint.contains(['#', '?']) {
        return Err(ClientError::UnsupportedFeature);
    }
    let (username, password) = credentials
        .split_once(':')
        .ok_or(ClientError::InvalidSubscription)?;
    let username = utf8_percent_encode(username, NON_ALPHANUMERIC);
    let password = utf8_percent_encode(password, NON_ALPHANUMERIC);
    parse_url(&format!(
        "{scheme}://{username}:{password}@{endpoint}{suffix}"
    ))
}

fn decode_base64_text(encoded: &str) -> Result<String, ClientError> {
    String::from_utf8(decode_base64(encoded)?).map_err(|_| ClientError::InvalidSubscription)
}

fn parse_url(text: &str) -> Result<ProxyProfile, ClientError> {
    let url = Url::parse(text).map_err(|_| ClientError::InvalidSubscription)?;
    if !matches!(url.path(), "" | "/") {
        return Err(ClientError::UnsupportedFeature);
    }
    if matches!(url.scheme(), "hy2" | "hysteria2") {
        return super::hysteria2::parse_link(&url);
    }
    let options = Options::parse(url.query().unwrap_or(""))?;
    let host = url.host_str().ok_or(ClientError::InvalidSubscription)?;
    let port = url
        .port_or_known_default()
        .or_else(|| match url.scheme() {
            "trojan" => Some(443),
            "socks" | "socks5" => Some(1080),
            _ => None,
        })
        .ok_or(ClientError::InvalidSubscription)?;
    make_profile(
        &decode_component(url.fragment().unwrap_or(""))?,
        host,
        port,
        parse_protocol(&url, &options)?,
        parse_transport(&options)?,
        parse_security(&url, &options, host)?,
    )
}

pub(super) struct Options(HashMap<String, String>);

impl Options {
    fn parse(query: &str) -> Result<Self, ClientError> {
        let allowed = [
            "type",
            "security",
            "sni",
            "fp",
            "peer",
            "pbk",
            "sid",
            "flow",
            "encryption",
            "path",
            "host",
            "allowInsecure",
            "alpn",
            "headerType",
            "mode",
            "extra",
        ];
        Self::parse_fields(query, |key| {
            allowed.contains(&key) || xhttp::is_xhttp_option(key)
        })
    }

    pub(super) fn parse_with(query: &str, allowed: &[&str]) -> Result<Self, ClientError> {
        Self::parse_fields(query, |key| allowed.contains(&key))
    }

    fn parse_fields(query: &str, allowed: impl Fn(&str) -> bool) -> Result<Self, ClientError> {
        let mut values = HashMap::new();
        for entry in query.split('&').filter(|entry| !entry.is_empty()) {
            let (key, value) = entry
                .split_once('=')
                .ok_or(ClientError::InvalidSubscription)?;
            let key = decode_component(key)?;
            if !allowed(&key) {
                return Err(ClientError::UnsupportedFeature);
            }
            if values.insert(key, decode_component(value)?).is_some() {
                return Err(ClientError::InvalidSubscription);
            }
        }
        Ok(Self(values))
    }

    pub(super) fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).map(String::as_str)
    }

    fn check(&self, key: &str, allowed: &[&str]) -> Result<(), ClientError> {
        if self.get(key).is_some_and(|value| !allowed.contains(&value)) {
            return Err(ClientError::UnsupportedFeature);
        }
        Ok(())
    }
}

pub(super) fn decode_component(text: &str) -> Result<String, ClientError> {
    let bytes = text.as_bytes();
    for (index, value) in bytes.iter().enumerate() {
        if *value == b'%'
            && !bytes
                .get(index + 1..index + 3)
                .is_some_and(|escape| escape.iter().all(u8::is_ascii_hexdigit))
        {
            return Err(ClientError::InvalidSubscription);
        }
    }
    percent_decode_str(text)
        .decode_utf8()
        .map(|value| value.into_owned())
        .map_err(|_| ClientError::InvalidSubscription)
}

fn parse_protocol(url: &Url, options: &Options) -> Result<ProxyProtocol, ClientError> {
    let username = decode_component(url.username())?;
    let password = url.password().map(decode_component).transpose()?;
    let flow = options.get("flow").unwrap_or("");
    if url.scheme() != "vless" && !flow.is_empty() {
        return Err(ClientError::UnsupportedFeature);
    }
    match url.scheme() {
        "vless" if password.is_none() => {
            vless(&username, flow, options.get("encryption").unwrap_or(""))
        }
        "vmess" if password.is_none() => {
            vmess(&username, options.get("encryption").unwrap_or("auto"))
        }
        "trojan" if password.is_none() => {
            options.check("encryption", &["none", ""])?;
            trojan(&username)
        }
        kind @ ("socks" | "socks5" | "http" | "https") => {
            options.check("encryption", &[])?;
            proxy_auth(
                kind,
                (!username.is_empty()).then_some(&username),
                password.as_deref(),
            )
        }
        "vless" | "vmess" | "trojan" => Err(ClientError::InvalidSubscription),
        _ => Err(ClientError::UnsupportedFormat),
    }
}

fn parse_transport(options: &Options) -> Result<ProxyTransport, ClientError> {
    options.check("headerType", &["none", ""])?;
    let kind = options.get("type").unwrap_or("tcp");
    if matches!(kind, "xhttp" | "splithttp") {
        let extra = options
            .get("extra")
            .filter(|value| !value.is_empty())
            .map(|value| parse_value(value).map_err(|_| ClientError::UnsupportedFeature))
            .transpose()?;
        let transport_options = xhttp::options(
            |key| options.get(key).map(xhttp::ValueRef::Text),
            extra.as_ref(),
        )?;
        return xhttp::transport(
            options.get("path").unwrap_or("/"),
            options.get("host"),
            options.get("mode"),
            transport_options,
        );
    }
    options.check("mode", &[""])?;
    options.check("extra", &[])?;
    if options.0.keys().any(|key| xhttp::is_xhttp_option(key)) {
        return Err(ClientError::UnsupportedFeature);
    }
    match kind {
        "tcp" | "raw" | "" => {
            options.check("path", &[""])?;
            options.check("host", &[""])?;
            Ok(ProxyTransport::Tcp)
        }
        "ws" | "websocket" => {
            xray_websocket(options.get("path").unwrap_or("/"), options.get("host"))
        }
        _ => Err(ClientError::UnsupportedFeature),
    }
}

fn parse_security(url: &Url, options: &Options, host: &str) -> Result<ProxySecurity, ClientError> {
    let protection = options.get("security").unwrap_or_else(|| {
        if matches!(url.scheme(), "trojan" | "https") {
            "tls"
        } else {
            "none"
        }
    });
    if url.scheme() == "https" && protection != "tls" {
        return Err(ClientError::UnsupportedFeature);
    }
    let (sni, peer) = (options.get("sni"), options.get("peer"));
    if sni.is_some() && peer.is_some() {
        return Err(ClientError::InvalidSubscription);
    }
    let server_name = sni.or(peer).unwrap_or(host);
    if protection != "reality" {
        options.check("pbk", &[])?;
        options.check("sid", &[])?;
    }
    let alpn = link_alpn(options.get("alpn"))?;
    if protection != "tls" && !alpn.is_empty() {
        return Err(ClientError::UnsupportedFeature);
    }
    let security = match protection {
        "none" | "" => {
            options.check("sni", &[])?;
            options.check("peer", &[])?;
            options.check("allowInsecure", &["0", "false"])?;
            Ok(ProxySecurity::None)
        }
        "tls" => verified_tls(server_name, insecure(options.get("allowInsecure"))?, alpn),
        "reality" => {
            options.check("allowInsecure", &["0", "false"])?;
            reality(
                server_name,
                options.get("pbk").ok_or(ClientError::InvalidSubscription)?,
                options.get("sid").unwrap_or(""),
            )
        }
        _ => Err(ClientError::UnsupportedFeature),
    }?;
    with_tls_fingerprint(security, options.get("fp"))
}

fn link_alpn(value: Option<&str>) -> Result<AlpnProtocols, ClientError> {
    let Some(value) = value.filter(|text| !text.is_empty()) else {
        return Ok(AlpnProtocols::default());
    };
    AlpnProtocols::new(value.split(',').map(str::to_owned).collect())
        .map_err(|_| ClientError::InvalidSubscription)
}

pub(super) fn insecure(value: Option<&str>) -> Result<bool, ClientError> {
    match value {
        None | Some("false" | "0") => Ok(false),
        Some("true" | "1") => Ok(true),
        _ => Err(ClientError::InvalidSubscription),
    }
}

fn parse_v2rayn(encoded: &str) -> Result<ProxyProfile, ClientError> {
    let bytes = decode_base64(encoded)?;
    let value = parse_value(super::document_text(&bytes)?)?;
    xhttp::fields_with_options(
        &value,
        &[
            "v",
            "ps",
            "add",
            "port",
            "id",
            "aid",
            "scy",
            "net",
            "type",
            "host",
            "path",
            "tls",
            "sni",
            "alpn",
            "fp",
            "allowInsecure",
            "mode",
            "extra",
        ],
    )?;
    check_empty(&value, "v", |field| {
        field.as_u64() == Some(2) || field.as_str() == Some("2")
    })?;
    check_empty(&value, "aid", |field| {
        zero(field) || field.as_str() == Some("")
    })?;
    check_empty(&value, "type", |value| {
        matches!(value.as_str(), Some("" | "none"))
    })?;
    let host = string(&value, "add")?;
    make_profile(
        optional_str(&value, "ps")?.unwrap_or(""),
        host,
        port(&value, "port")?,
        vmess(
            string(&value, "id")?,
            optional_str(&value, "scy")?.unwrap_or("auto"),
        )?,
        v2rayn_transport(&value)?,
        with_tls_fingerprint(v2rayn_security(&value, host)?, optional_str(&value, "fp")?)?,
    )
}

fn v2rayn_transport(value: &Value) -> Result<ProxyTransport, ClientError> {
    let host = optional_str(value, "host")?.filter(|field| !field.is_empty());
    let path = optional_str(value, "path")?.filter(|field| !field.is_empty());
    let kind = optional_str(value, "net")?.unwrap_or("tcp");
    let mode = optional_str(value, "mode")?.filter(|field| !field.is_empty());
    if matches!(kind, "xhttp" | "splithttp") {
        return xhttp::transport(path.unwrap_or("/"), host, mode, xhttp::json_options(value)?);
    }
    if mode.is_some()
        || value.get("extra").is_some()
        || value
            .as_object()
            .is_some_and(|object| object.keys().any(|key| xhttp::is_xhttp_option(key)))
    {
        return Err(ClientError::UnsupportedFeature);
    }
    match kind {
        "tcp" | "raw" | "" if host.is_none() && path.is_none() => Ok(ProxyTransport::Tcp),
        "ws" | "websocket" => xray_websocket(path.unwrap_or("/"), host),
        _ => Err(ClientError::UnsupportedFeature),
    }
}

fn v2rayn_security(value: &Value, host: &str) -> Result<ProxySecurity, ClientError> {
    let sni = optional_str(value, "sni")?.filter(|field| !field.is_empty());
    let alpn = link_alpn(optional_str(value, "alpn")?)?;
    let insecure = match value.get("allowInsecure") {
        Some(Value::Bool(value)) => *value,
        Some(Value::Number(value)) if matches!(value.as_u64(), Some(0 | 1)) => {
            value.as_u64() == Some(1)
        }
        Some(Value::String(value)) if value.is_empty() => false,
        Some(Value::String(value)) => insecure(Some(value))?,
        None => false,
        _ => return Err(ClientError::InvalidSubscription),
    };
    match optional_str(value, "tls")?.unwrap_or("") {
        "none" | "" if sni.is_none() && !insecure && alpn.is_empty() => Ok(ProxySecurity::None),
        "tls" => verified_tls(sni.unwrap_or(host), insecure, alpn),
        _ => Err(ClientError::UnsupportedFeature),
    }
}

fn parse_shadowsocks(encoded: &str) -> Result<ProxyProfile, ClientError> {
    let (body, label) = encoded.split_once('#').unwrap_or((encoded, ""));
    if body.contains('?') {
        return Err(ClientError::UnsupportedFeature);
    }
    let legacy = !body.contains('@');
    let decoded;
    let body = if !legacy {
        body
    } else {
        decoded = String::from_utf8(decode_base64(body)?)
            .map_err(|_| ClientError::InvalidSubscription)?;
        &decoded
    };
    let (credentials, endpoint) = body
        .rsplit_once('@')
        .ok_or(ClientError::InvalidSubscription)?;
    let credentials = if legacy {
        credentials.to_owned()
    } else {
        decode_sip002_credentials(credentials)?
    };
    let (cipher, password) = credentials
        .split_once(':')
        .ok_or(ClientError::InvalidSubscription)?;
    let url = Url::parse(&format!("socks5://{endpoint}"))
        .map_err(|_| ClientError::InvalidSubscription)?;
    if !matches!(url.path(), "" | "/") || url.query().is_some() || url.fragment().is_some() {
        return Err(ClientError::UnsupportedFeature);
    }
    make_profile(
        &decode_component(label)?,
        url.host_str().ok_or(ClientError::InvalidSubscription)?,
        url.port().ok_or(ClientError::InvalidSubscription)?,
        shadowsocks(cipher, password)?,
        ProxyTransport::Tcp,
        ProxySecurity::None,
    )
}

fn decode_sip002_credentials(credentials: &str) -> Result<String, ClientError> {
    let credentials = decode_component(credentials)?;
    if credentials.contains(':') {
        Ok(credentials)
    } else {
        String::from_utf8(decode_base64(&credentials)?)
            .map_err(|_| ClientError::InvalidSubscription)
    }
}
