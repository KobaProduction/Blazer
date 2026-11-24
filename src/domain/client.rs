//! Typed client catalog. Secrets are redacted from Debug and require explicit access.

use std::collections::HashSet;
use std::fmt;
use std::num::NonZeroU16;
use std::sync::Arc;

mod graph;
mod probe;
mod tun_conflict;
pub use graph::{
    GraphGroup, GraphMatch, GraphNetwork, GraphObservatory, GraphOutbound, GraphOutboundKind,
    GraphRule, GraphStrategy, GraphTarget, ProbeReport, ProxyGraph, record_graph_probe,
    select_least_ping,
};
pub use tun_conflict::{TunAdapterKind, TunConflict};
mod xhttp;
pub use probe::{ProbeHttpMethod, ProbeMethod, ProbeSettings, ProbeStrategy};
mod xray_tls_profiles;
pub use xhttp::{
    XhttpMetadata, XhttpPacketOptions, XhttpPacketRange, XhttpPlacement, XhttpWireField,
    XhttpWireOptions, XhttpWireValue,
};

pub const MAX_PROFILES: usize = 256;
pub const MAX_SUBSCRIPTIONS: usize = 64;
pub const MAX_SUBSCRIPTION_USER_AGENT_BYTES: usize = 512;
pub const DEFAULT_SUBSCRIPTION_USER_AGENT: &str = match option_env!("BLAZER_RELEASE_USER_AGENT") {
    Some(agent) => agent,
    None => concat!(
        "Blazer/",
        env!("CARGO_PKG_VERSION_MAJOR"),
        ".",
        env!("CARGO_PKG_VERSION_MINOR")
    ),
};

#[derive(Clone, PartialEq, Eq)]
pub struct Secret(Arc<str>);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(Arc::from(value.into()))
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[redacted]")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Language {
    #[default]
    Russian,
    English,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConnectionMode {
    #[default]
    LocalProxy,
    SystemProxy,
    Tun,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientError {
    InvalidInput,
    UnsupportedFormat,
    UnsupportedFeature,
    ProbeConnectionNotReusable,
    UnsupportedXhttpUploadMethod,
    PayloadTooLarge,
    InvalidUrl,
    Network,
    SubscriptionHttp(SubscriptionHttpError),
    Timeout,
    Tls,
    InvalidSubscription,
    StorageUnavailable,
    StorageCorrupt,
    StorageWrite,
    NoProfileSelected,
    ProfileNotFound,
    RoutingProfileMissing,
    RoutingProfileAssigned,
    SubscriptionNotFound,
    EngineNotFound,
    EngineUntrusted,
    EngineInvalid,
    EngineStart,
    SystemProxyUnavailable,
    SystemProxyRestoreFailed,
    TunServiceUnavailable,
    TunConflict(TunConflict),
    TunPreflightUnavailable,
    LinuxTunUnavailable,
    EngineReadiness,
    EngineStopped,
    UnsupportedMode,
    Cancelled,
    DisconnectFirst,
    ShutdownTimeout,
    TelemetryEndpointUnavailable,
}

const HTTP_ERROR_DETAIL_BYTES: usize = 96;

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct SubscriptionHttpError {
    status: u16,
    detail: [u8; HTTP_ERROR_DETAIL_BYTES],
    detail_len: u8,
}

impl SubscriptionHttpError {
    pub fn new(status: u16, detail: &str) -> Self {
        let end = detail.floor_char_boundary(HTTP_ERROR_DETAIL_BYTES);
        let mut bytes = [0; HTTP_ERROR_DETAIL_BYTES];
        bytes[..end].copy_from_slice(&detail.as_bytes()[..end]);
        Self {
            status,
            detail: bytes,
            detail_len: end as u8,
        }
    }

    pub fn status(&self) -> u16 {
        self.status
    }

    pub fn detail(&self) -> Option<&str> {
        let detail = std::str::from_utf8(&self.detail[..usize::from(self.detail_len)]).ok()?;
        (!detail.is_empty()).then_some(detail)
    }
}

impl fmt::Debug for SubscriptionHttpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SubscriptionHttpError")
            .field("status", &self.status)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProxyProtocol {
    Graph(Box<ProxyGraph>),
    Native(NativeProxy),
    Socks5 {
        username: Option<Secret>,
        password: Option<Secret>,
    },
    Http {
        username: Option<Secret>,
        password: Option<Secret>,
    },
    Vless {
        user_uuid: Secret,
        vision: bool,
        encryption: Option<Secret>,
    },
    Vmess {
        user_uuid: Secret,
        cipher: String,
    },
    Trojan {
        password: Secret,
    },
    Hysteria2 {
        password: Secret,
        salamander_password: Option<Secret>,
        udp_enabled: bool,
    },
    Shadowsocks {
        cipher: String,
        password: Secret,
    },
}

impl ProxyProtocol {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Graph(_) => "Xray graph",
            Self::Native(proxy) => proxy.kind.name(),
            Self::Socks5 { .. } => "SOCKS5",
            Self::Http { .. } => "HTTP",
            Self::Vless { .. } => "VLESS",
            Self::Vmess { .. } => "VMess",
            Self::Trojan { .. } => "Trojan",
            Self::Hysteria2 { .. } => "Hysteria2",
            Self::Shadowsocks { .. } => "Shadowsocks",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeProxy {
    pub kind: NativeProxyKind,
    pub settings: Secret,
    pub dns_servers: Vec<std::net::IpAddr>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeProxyKind {
    WireGuard,
    AmneziaWg,
    Hysteria2,
    Tuic,
    Http,
    Socks5,
    Shadowsocks,
    Snell,
    Vless,
    Vmess,
    Trojan,
    AnyTls,
    NaiveProxy,
    PortForward,
}

impl NativeProxyKind {
    pub const fn name(self) -> &'static str {
        match self {
            Self::WireGuard => "WireGuard",
            Self::AmneziaWg => "AmneziaWG",
            Self::Hysteria2 => "Hysteria2",
            Self::Tuic => "TUIC",
            Self::Http => "HTTP",
            Self::Socks5 => "SOCKS5",
            Self::Shadowsocks => "Shadowsocks",
            Self::Snell => "Snell",
            Self::Vless => "VLESS",
            Self::Vmess => "VMess",
            Self::Trojan => "Trojan",
            Self::AnyTls => "AnyTLS",
            Self::NaiveProxy => "NaiveProxy",
            Self::PortForward => "Port forward",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProxyTransport {
    Tcp,
    Udp,
    WebSocket {
        path: String,
        host: Option<String>,
        early_data: Option<WebSocketEarlyData>,
    },
    Xhttp {
        path: String,
        host: Option<String>,
        mode: XhttpMode,
        metadata: XhttpMetadata,
        packet: XhttpPacketOptions,
    },
    Quic {
        certificate_sha256: Option<String>,
    },
}

impl ProxyTransport {
    pub const fn has_tcp_endpoint(&self) -> bool {
        !matches!(self, Self::Udp | Self::Quic { .. })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebSocketEarlyData {
    pub max_bytes: NonZeroU16,
    pub header_name: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XhttpMode {
    Auto,
    PacketUp,
    StreamUp,
    StreamOne,
}

impl XhttpMode {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::PacketUp => "packet-up",
            Self::StreamUp => "stream-up",
            Self::StreamOne => "stream-one",
        }
    }
}

impl std::str::FromStr for XhttpMode {
    type Err = ClientError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "auto" => Ok(Self::Auto),
            "packet-up" => Ok(Self::PacketUp),
            "stream-up" => Ok(Self::StreamUp),
            "stream-one" => Ok(Self::StreamOne),
            _ => Err(ClientError::UnsupportedFeature),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProxySecurity {
    None,
    Tls {
        server_name: String,
        allow_insecure: bool,
        alpn: AlpnProtocols,
        fingerprint: Option<TlsFingerprint>,
    },
    Reality {
        server_name: String,
        public_key: String,
        short_id: String,
        fingerprint: Option<TlsFingerprint>,
    },
}

impl ProxySecurity {
    pub fn fingerprint(&self) -> Option<TlsFingerprint> {
        match self {
            Self::Tls { fingerprint, .. } | Self::Reality { fingerprint, .. } => *fingerprint,
            Self::None => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TlsFingerprint {
    Firefox120,
    Firefox148,
    Xray(&'static str),
}

impl TlsFingerprint {
    pub fn parse(text: &str) -> Result<Self, ClientError> {
        match text.to_ascii_lowercase().as_str() {
            "firefox-120" | "firefox_120" | "hellofirefox_120" => Ok(Self::Firefox120),
            "firefox" | "firefox-148" | "firefox_148" | "firefox_auto" | "hellofirefox_auto"
            | "hellofirefox_148" => Ok(Self::Firefox148),
            name => xray_tls_profiles::canonical_name(name)
                .map(Self::Xray)
                .ok_or(ClientError::UnsupportedFeature),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Firefox120 => "firefox-120",
            Self::Firefox148 => "firefox-148",
            Self::Xray(name) => name,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AlpnProtocols(Vec<String>);

impl AlpnProtocols {
    pub fn new(protocols: Vec<String>) -> Result<Self, ClientError> {
        // TLS ALPN names have a one-byte length within a two-byte length-prefixed list.
        let encoded_len = protocols.iter().try_fold(2usize, |total, protocol| {
            if !(1..=usize::from(u8::MAX)).contains(&protocol.len()) {
                return Err(ClientError::InvalidInput);
            }
            Ok(total + 1 + protocol.len())
        })?;
        if encoded_len > usize::from(u16::MAX) {
            return Err(ClientError::InvalidInput);
        }
        Ok(Self(protocols))
    }

    pub fn as_slice(&self) -> &[String] {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxyProfile {
    pub profile_uuid: String,
    pub name: String,
    pub source: Option<Secret>,
    pub host: String,
    pub port: u16,
    pub protocol: ProxyProtocol,
    pub transport: ProxyTransport,
    pub security: ProxySecurity,
}

impl ProxyProfile {
    pub fn same_identity(&self, other: &Self) -> bool {
        self.name == other.name
            && self.host == other.host
            && self.port == other.port
            && self.protocol.name() == other.protocol.name()
    }

    pub fn same_connection_settings(&self, other: &Self) -> bool {
        self.protocol == other.protocol
            && self.transport == other.transport
            && self.security == other.security
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogProfile {
    pub profile: ProxyProfile,
    pub subscription_uuid: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileUnavailableReason {
    Unsupported,
    UnsupportedXhttpUploadMethod,
    Invalid,
}

impl ProfileUnavailableReason {
    pub fn supports_tcp_probe(self) -> bool {
        matches!(self, Self::Unsupported | Self::UnsupportedXhttpUploadMethod)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnavailableProfile {
    pub name: String,
    pub protocol: Option<String>,
    pub reason: ProfileUnavailableReason,
    pub probe_id: Option<String>,
    probe_endpoint: Option<TcpProbeEndpoint>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TcpProbeEndpoint {
    host: Secret,
    port: NonZeroU16,
}

impl TcpProbeEndpoint {
    pub fn new(host: &str, port: u16) -> Option<Self> {
        if host.parse::<std::net::IpAddr>().is_err() && !valid_dns_probe_host(host) {
            return None;
        }
        Some(Self {
            host: Secret::new(host.to_ascii_lowercase()),
            port: NonZeroU16::new(port)?,
        })
    }

    pub fn address(&self) -> (&str, u16) {
        (self.host.expose(), self.port.get())
    }
}

fn valid_dns_probe_host(host: &str) -> bool {
    let host = host.strip_suffix('.').unwrap_or(host);
    !host.is_empty()
        && host.len() <= 253
        && host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
}

impl UnavailableProfile {
    pub fn new(name: &str, protocol: Option<&str>, reason: ProfileUnavailableReason) -> Self {
        let name = name.trim();
        let name = if valid_unavailable_name(name) {
            name
        } else {
            ""
        };
        Self {
            name: name.to_owned(),
            protocol: protocol
                .filter(|value| valid_protocol_label(value))
                .map(str::to_owned),
            reason,
            probe_id: None,
            probe_endpoint: None,
        }
    }

    pub fn with_probe_endpoint(mut self, endpoint: Option<TcpProbeEndpoint>) -> Self {
        if self.reason.supports_tcp_probe() {
            self.probe_endpoint = endpoint;
        }
        self
    }

    pub fn probe_endpoint(&self) -> Option<&TcpProbeEndpoint> {
        self.probe_endpoint.as_ref()
    }

    pub fn for_display(&self) -> Self {
        Self {
            probe_endpoint: None,
            ..self.clone()
        }
    }

    fn validate(&self) -> Result<(), ClientError> {
        if !valid_unavailable_name(&self.name)
            || self
                .protocol
                .as_deref()
                .is_some_and(|label| !valid_protocol_label(label))
        {
            return Err(ClientError::InvalidInput);
        }
        match (&self.probe_id, &self.probe_endpoint) {
            (None, None) => Ok(()),
            (Some(uuid), Some(_))
                if self.reason.supports_tcp_probe() && !uuid.is_empty() && uuid.len() <= 64 =>
            {
                Ok(())
            }
            _ => Err(ClientError::InvalidInput),
        }
    }
}

fn valid_unavailable_name(name: &str) -> bool {
    name.len() <= 256 && !name.chars().any(char::is_control) && !name.contains("://")
}

fn valid_protocol_label(label: &str) -> bool {
    !label.is_empty()
        && label.len() <= 32
        && label
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Subscription {
    pub subscription_uuid: String,
    pub title: String,
    pub auto_title: bool,
    pub url: Secret,
    pub provider: crate::provider::ProviderState,
    pub unavailable_profiles: Vec<UnavailableProfile>,
}

impl Subscription {
    /// An empty title opts into the subscription's automatic name.
    pub fn validate_title_override(title: &str) -> Result<(), ClientError> {
        if title.is_empty() {
            Ok(())
        } else {
            Self::validate_title(title)
        }
    }

    pub fn validate_title(title: &str) -> Result<(), ClientError> {
        if title.trim().is_empty() || title.len() > 256 || title.chars().any(char::is_control) {
            return Err(ClientError::InvalidInput);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TelemetrySettings {
    pub mode: TelemetryMode,
    pub endpoint: Option<String>,
    pub bearer_token: Option<Secret>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum TelemetryMode {
    #[default]
    Default,
    Disabled,
    Manual,
}

impl TelemetrySettings {
    pub fn validate(&self) -> Result<(), ClientError> {
        if self.mode == TelemetryMode::Manual && self.endpoint.is_none() {
            return Err(ClientError::InvalidUrl);
        }
        if let Some(endpoint) = self.endpoint.as_deref() {
            Self::validate_endpoint(endpoint)?;
        }
        if let Some(bearer) = &self.bearer_token {
            Self::validate_bearer_token(bearer.expose())?;
        }
        Ok(())
    }

    pub fn validate_bearer_token(token: &str) -> Result<(), ClientError> {
        if token.is_empty()
            || token.len() > 4096
            || !token.bytes().all(|byte| byte.is_ascii_graphic())
        {
            return Err(ClientError::InvalidInput);
        }
        Ok(())
    }

    pub fn validate_endpoint(endpoint: &str) -> Result<(), ClientError> {
        let (scheme, location) = endpoint.split_once("://").ok_or(ClientError::InvalidUrl)?;
        let authority = location.split('/').next().unwrap_or_default();
        if !["http", "https"]
            .iter()
            .any(|name| scheme.eq_ignore_ascii_case(name))
            || authority.is_empty()
            || authority.contains('@')
            || endpoint.contains(['?', '#', '\\'])
            || endpoint.len() > crate::provider::MAX_METADATA_VALUE_BYTES
            || endpoint
                .chars()
                .any(|ch| ch.is_whitespace() || ch.is_control())
        {
            return Err(ClientError::InvalidUrl);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientSettings {
    pub probe: ProbeSettings,
    pub language: Language,
    pub local_proxy_port: u16,
    pub connection_mode: ConnectionMode,
    pub send_subscription_hwid: bool,
    pub subscription_user_agent: Option<Secret>,
    pub telemetry: TelemetrySettings,
}

pub fn validate_subscription_user_agent(value: &str) -> Result<(), ClientError> {
    if value.len() > MAX_SUBSCRIPTION_USER_AGENT_BYTES {
        return Err(ClientError::PayloadTooLarge);
    }
    if value.is_empty()
        || value.trim() != value
        || !value.bytes().all(|byte| matches!(byte, b' '..=b'~'))
    {
        return Err(ClientError::InvalidInput);
    }
    Ok(())
}

impl Default for ClientSettings {
    fn default() -> Self {
        Self {
            probe: ProbeSettings::default(),
            language: Language::Russian,
            local_proxy_port: 0,
            connection_mode: ConnectionMode::LocalProxy,
            send_subscription_hwid: true,
            subscription_user_agent: None,
            telemetry: TelemetrySettings::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Catalog {
    pub subscriptions: Vec<Subscription>,
    pub profiles: Vec<CatalogProfile>,
    pub selected_profile_uuid: Option<String>,
    pub settings: ClientSettings,
    pub routing: crate::routing::RoutingCatalog,
}

impl Catalog {
    pub fn validate(&self) -> Result<(), ClientError> {
        if self.profile_count() > MAX_PROFILES || self.subscriptions.len() > MAX_SUBSCRIPTIONS {
            return Err(ClientError::PayloadTooLarge);
        }
        let mut endpoint_count = 0usize;
        for item in &self.profiles {
            let count = if let ProxyProtocol::Graph(graph) = &item.profile.protocol {
                graph.validate()?;
                graph.outbounds.len()
            } else {
                1
            };
            endpoint_count = endpoint_count.saturating_add(count);
        }
        if endpoint_count > MAX_PROFILES {
            return Err(ClientError::PayloadTooLarge);
        }
        for subscription in &self.subscriptions {
            Subscription::validate_title(&subscription.title)?;
            subscription.provider.metadata.validate()?;
            for profile in &subscription.unavailable_profiles {
                profile.validate()?;
            }
        }
        if let Some(user_agent) = &self.settings.subscription_user_agent {
            validate_subscription_user_agent(user_agent.expose())?;
        }
        self.settings.telemetry.validate()?;
        self.settings.probe.validate()?;
        self.routing.validate(&self.subscriptions)
    }

    pub fn profile_count(&self) -> usize {
        self.subscriptions
            .iter()
            .fold(self.profiles.len(), |count, subscription| {
                count.saturating_add(subscription.unavailable_profiles.len())
            })
    }

    pub fn repair_selection(&mut self) {
        let selected_exists = self.selected_profile_uuid.as_ref().is_some_and(|selected| {
            self.profiles
                .iter()
                .any(|item| &item.profile.profile_uuid == selected)
        });
        if !selected_exists {
            self.selected_profile_uuid = self
                .profiles
                .first()
                .map(|item| item.profile.profile_uuid.clone());
        }
    }
}

pub fn preserve_profile_uuids(profiles: &mut [ProxyProfile], previous: &[ProxyProfile]) {
    let mut matched = HashSet::new();
    // Match full settings first so reordered variants keep their own identities.
    for exact in [true, false] {
        for profile in profiles
            .iter_mut()
            .filter(|item| item.profile_uuid.is_empty())
        {
            if let Some(old) = previous.iter().find(|old| {
                !matched.contains(old.profile_uuid.as_str())
                    && old.same_identity(profile)
                    && (!exact || old.same_connection_settings(profile))
            }) {
                matched.insert(old.profile_uuid.as_str());
                profile.profile_uuid = old.profile_uuid.clone();
            }
        }
    }
}
