use serde_json::Value;
use url::Url;

use super::json::{fields, optional_str, port, string};
use super::links::{Options, decode_component, insecure};
use super::{
    AlpnProtocols, ClientError, ProxyProfile, ProxyProtocol, ProxySecurity, ProxyTransport, Secret,
    make_profile, verified_tls,
};

pub(super) fn parse_link(url: &Url) -> Result<ProxyProfile, ClientError> {
    let options = Options::parse_with(
        url.query().unwrap_or(""),
        &["sni", "insecure", "pinSHA256", "obfs", "obfs-password"],
    )?;
    let host = url.host_str().ok_or(ClientError::InvalidSubscription)?;
    let mut password = decode_component(url.username())?;
    if let Some(suffix) = url.password() {
        password.push(':');
        password.push_str(&decode_component(suffix)?);
    }
    let obfs = salamander(options.get("obfs"), options.get("obfs-password"))?;
    let certificate_sha256 = options.get("pinSHA256").map(certificate_pin).transpose()?;
    let security = verified_tls(
        options
            .get("sni")
            .filter(|name| !name.is_empty())
            .unwrap_or(host),
        insecure(options.get("insecure"))?,
        AlpnProtocols::default(),
    )?;
    make_profile(
        &decode_component(url.fragment().unwrap_or(""))?,
        host,
        url.port().unwrap_or(443),
        protocol(&password, obfs, true)?,
        ProxyTransport::Quic { certificate_sha256 },
        security,
    )
}

pub(super) fn parse_sing_box(value: &Value) -> Result<ProxyProfile, ClientError> {
    fields(
        value,
        &[
            "type",
            "tag",
            "server",
            "server_port",
            "password",
            "tls",
            "obfs",
            "network",
        ],
    )?;
    let host = string(value, "server")?;
    let tls = super::sing_box::parse_tls(
        value.get("tls").ok_or(ClientError::InvalidSubscription)?,
        host,
    )?;
    if !matches!(tls, ProxySecurity::Tls { .. }) {
        return Err(ClientError::UnsupportedFeature);
    }
    let udp_enabled = match optional_str(value, "network")? {
        None | Some("") => true,
        Some("tcp") => false,
        Some(_) => return Err(ClientError::UnsupportedFeature),
    };
    let obfs = value.get("obfs").map(parse_obfs).transpose()?.flatten();
    make_profile(
        optional_str(value, "tag")?.unwrap_or(""),
        host,
        port(value, "server_port")?,
        protocol(string(value, "password")?, obfs, udp_enabled)?,
        ProxyTransport::Quic {
            certificate_sha256: None,
        },
        tls,
    )
}

fn parse_obfs(value: &Value) -> Result<Option<Secret>, ClientError> {
    fields(value, &["type", "password"])?;
    salamander(
        optional_str(value, "type")?,
        optional_str(value, "password")?,
    )
}

fn salamander(kind: Option<&str>, password: Option<&str>) -> Result<Option<Secret>, ClientError> {
    match (kind, password) {
        (None | Some(""), None | Some("")) => Ok(None),
        (Some("salamander"), Some(password)) if password.len() >= 4 => {
            Ok(Some(Secret::new(password)))
        }
        (Some("salamander") | None | Some(""), _) => Err(ClientError::InvalidSubscription),
        _ => Err(ClientError::UnsupportedFeature),
    }
}

fn protocol(
    password: &str,
    salamander_password: Option<Secret>,
    udp_enabled: bool,
) -> Result<ProxyProtocol, ClientError> {
    if password.is_empty() {
        return Err(ClientError::InvalidSubscription);
    }
    Ok(ProxyProtocol::Hysteria2 {
        password: Secret::new(password),
        salamander_password,
        udp_enabled,
    })
}

fn certificate_pin(value: &str) -> Result<String, ClientError> {
    let pin = value.replace(':', "");
    if pin.len() != 64 || !pin.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(ClientError::InvalidSubscription);
    }
    Ok(pin.to_ascii_lowercase())
}
