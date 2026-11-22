use vpn_application::client::{
    NormalizedSubscription, ProfileUnavailableReason, UnavailableProfile,
};

use super::{ClientError, MAX_PROFILES, ProxyProfile, json, links, parse_input};

pub(super) fn parse(input: &[u8]) -> Result<NormalizedSubscription, ClientError> {
    parse_input(input, parse_document)
}

fn parse_document(text: &str) -> Result<NormalizedSubscription, ClientError> {
    let mut profiles = SubscriptionProfiles::default();
    if super::wireguard::recognizes(text) {
        profiles.append(super::wireguard::parse(text), true, |reason| {
            UnavailableProfile::new("", Some("wireguard"), reason)
        })?;
    } else if text.starts_with(['{', '[']) || json::recognizes_native_yaml(text) {
        json::append_subscription_entries(text, &mut profiles)?;
    } else {
        for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
            if link_scheme(line).is_none() {
                return Err(ClientError::InvalidSubscription);
            }
            profiles.append_link(line)?;
        }
    }
    if !profiles.has_candidate {
        return Err(ClientError::InvalidSubscription);
    }
    Ok(profiles.result)
}

#[derive(Default)]
pub(super) struct SubscriptionProfiles {
    result: NormalizedSubscription,
    has_candidate: bool,
}

impl SubscriptionProfiles {
    pub(super) fn append_link(&mut self, link: &str) -> Result<(), ClientError> {
        let scheme = link_scheme(link);
        let protocol = scheme.and_then(protocol_label);
        let result = match (scheme, protocol) {
            (
                _,
                Some(
                    "vless" | "vmess" | "trojan" | "socks5" | "http" | "shadowsocks" | "hysteria2"
                    | "amnezia",
                ),
            ) => links::parse_share_link(link),
            (Some(_), _) => Err(ClientError::UnsupportedFormat),
            _ => Err(ClientError::InvalidSubscription),
        };
        self.append(result, protocol.is_some(), |reason| {
            let fragment = link
                .split_once('#')
                .filter(|_| protocol.is_some())
                .and_then(|(_, name)| links::decode_component(name).ok());
            let endpoint = reason
                .supports_tcp_probe()
                .then(|| links::tcp_probe_endpoint(link))
                .flatten();
            UnavailableProfile::new(fragment.as_deref().unwrap_or_default(), protocol, reason)
                .with_probe_endpoint(endpoint)
        })
    }

    pub(super) fn append(
        &mut self,
        result: Result<ProxyProfile, ClientError>,
        is_candidate: bool,
        metadata: impl FnOnce(ProfileUnavailableReason) -> UnavailableProfile,
    ) -> Result<(), ClientError> {
        if self.result.profiles.len() + self.result.unavailable_profiles.len() == MAX_PROFILES {
            return Err(ClientError::PayloadTooLarge);
        }
        match result {
            Ok(profile) => self.result.profiles.push(profile),
            Err(error) => {
                let reason = match error {
                    ClientError::UnsupportedFeature | ClientError::UnsupportedFormat => {
                        ProfileUnavailableReason::Unsupported
                    }
                    ClientError::UnsupportedXhttpUploadMethod => {
                        ProfileUnavailableReason::UnsupportedXhttpUploadMethod
                    }
                    ClientError::InvalidSubscription => ProfileUnavailableReason::Invalid,
                    error => return Err(error),
                };
                self.result.unavailable_profiles.push(metadata(reason));
            }
        }
        self.has_candidate |= is_candidate;
        Ok(())
    }
}

fn link_scheme(link: &str) -> Option<&str> {
    let (scheme, _) = link.split_once("://")?;
    let (first, rest) = scheme.as_bytes().split_first()?;
    (first.is_ascii_alphabetic()
        && rest
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || b"+.-".contains(byte)))
    .then_some(scheme)
}

pub(super) fn is_tcp_transport(transport: &str) -> bool {
    matches!(
        transport,
        "tcp" | "raw" | "ws" | "http" | "h2" | "grpc" | "httpupgrade" | "xhttp" | "splithttp"
    )
}

pub(super) fn protocol_label(protocol: &str) -> Option<&'static str> {
    if protocol.len() > 16 {
        return None;
    }
    match protocol.to_ascii_lowercase().as_str() {
        "vless" => Some("vless"),
        "vmess" => Some("vmess"),
        "trojan" => Some("trojan"),
        "socks" | "socks5" => Some("socks5"),
        "http" | "https" => Some("http"),
        "ss" | "shadowsocks" => Some("shadowsocks"),
        "hy2" | "hysteria2" => Some("hysteria2"),
        "hysteria" => Some("hysteria"),
        "ssr" => Some("ssr"),
        "tuic" => Some("tuic"),
        "wireguard" | "wg" => Some("wireguard"),
        "vpn" => Some("amnezia"),
        "amneziawg" | "awg" => Some("amneziawg"),
        "snell" => Some("snell"),
        "anytls" => Some("anytls"),
        "naiveproxy" => Some("naive"),
        "mieru" => Some("mieru"),
        "naive" => Some("naive"),
        "ssh" => Some("ssh"),
        _ => None,
    }
}
