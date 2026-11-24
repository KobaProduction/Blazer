//! Typed commands and state shared by clients across platforms.

use std::collections::HashMap;
use std::fmt;

use vpn_domain::client::TelemetrySettings;
pub use vpn_domain::client::{
    ClientError, ConnectionMode, DEFAULT_SUBSCRIPTION_USER_AGENT, Language,
    MAX_SUBSCRIPTION_USER_AGENT_BYTES, ProbeHttpMethod, ProbeMethod, ProbeSettings, ProbeStrategy,
    ProfileUnavailableReason, Secret, TelemetryMode, TunAdapterKind, TunConflict,
    UnavailableProfile, validate_subscription_user_agent,
};
pub use vpn_domain::provider::{
    ProviderChangeKind, ProviderKey, ProviderMetadataClass, SubscriptionUsage,
};
pub use vpn_domain::routing::{
    DnsPolicy, RouteAction, RouteRule, RouteTarget, RoutingAssignment, RoutingCatalog,
    RoutingInput, RoutingProfile,
};

/// Nonblocking session operations consumed by presentation adapters.
pub trait ClientSession: Send {
    fn snapshot(&self) -> ClientSnapshot;
    fn send(&self, command: ClientCommand) -> Result<RequestId, SubmitError>;
    fn shutdown(&mut self) -> Result<(), ClientError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiagnosticSummary {
    pub buffered: usize,
    pub dropped: u64,
}

pub enum ClientCommand {
    AddSubscription {
        title: String,
        url: String,
    },
    UpdateSubscription {
        subscription_uuid: String,
        title: String,
        url: String,
    },
    RenameSubscription {
        subscription_uuid: String,
        title: String,
    },
    RefreshSubscription {
        subscription_uuid: String,
    },
    ApproveProviderChange {
        subscription_uuid: String,
        nonce: String,
    },
    RejectProviderChange {
        subscription_uuid: String,
        nonce: String,
    },
    SetSubscriptionFallback {
        subscription_uuid: String,
        url: Option<String>,
    },
    RemoveSubscription {
        subscription_uuid: String,
    },
    ImportText {
        title: String,
        text: String,
    },
    UpdateProfile {
        profile_uuid: String,
        text: Secret,
        title: Option<String>,
    },
    RenameProfile {
        profile_uuid: String,
        title: String,
    },
    SelectProfile {
        profile_uuid: String,
    },
    ProbeProfile {
        profile_uuid: String,
    },
    ProbeProfiles {
        subscription_uuid: Option<String>,
    },
    RemoveProfile {
        profile_uuid: String,
    },
    Connect {
        mode: ConnectionMode,
    },
    ReconnectRouting {
        pending: RoutingReconnect,
    },
    Disconnect,
    SetLanguage {
        language: Language,
    },
    SetConnectionMode {
        mode: ConnectionMode,
    },
    SetLocalProxyPort {
        port: u16,
    },
    SetProbeSettings {
        settings: ProbeSettings,
    },
    SetSubscriptionHwid {
        enabled: bool,
    },
    SetSubscriptionUserAgent {
        value: Option<Secret>,
    },
    SetTelemetrySettings {
        mode: TelemetryMode,
        endpoint: String,
        bearer: TelemetryBearerChange,
    },
    SaveRoutingProfile {
        uuid: Option<String>,
        profile: RoutingInput,
    },
    RemoveRoutingProfile {
        profile_uuid: String,
    },
    SetRoutingAssignment {
        subscription_uuid: Option<String>,
        assignment: RoutingAssignment,
    },
    SetDnsPolicy {
        profile_uuid: String,
        policy: DnsPolicy,
    },
}

#[derive(Clone, Default, PartialEq, Eq)]
pub enum TelemetryBearerChange {
    #[default]
    Keep,
    Clear,
    Replace(String),
}

impl fmt::Debug for TelemetryBearerChange {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Keep => "Keep",
            Self::Clear => "Clear",
            Self::Replace(_) => "Replace([redacted])",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubmitError {
    Busy,
    Stopped,
    Rejected(ClientError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionStatus {
    Disconnected,
    Connecting,
    Connected,
    Disconnecting,
    Failed,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ModeCapabilities {
    pub local_proxy: bool,
    pub local_http_proxy: bool,
    pub system_proxy: bool,
    pub tun: bool,
}

impl ModeCapabilities {
    pub const fn supports(self, mode: ConnectionMode) -> bool {
        match mode {
            ConnectionMode::LocalProxy => self.local_proxy || self.local_http_proxy,
            ConnectionMode::SystemProxy => self.system_proxy,
            ConnectionMode::Tun => self.tun,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SubscriptionSummary {
    pub subscription_uuid: String,
    pub title: String,
    pub auto_title: bool,
    pub url: Secret,
    pub profile_count: usize,
    pub last_error: Option<ClientError>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderChangePreview {
    pub kind: ProviderChangeKind,
    pub nonce: String,
    pub from_origin: String,
    pub target_origin: String,
}

#[derive(Debug, Clone)]
pub struct ProviderSubscriptionSummary {
    pub subscription_uuid: String,
    pub raw_config_hidden: bool,
    pub has_committed_fallback: bool,
    pub last_success_unix: Option<u64>,
    pub last_update_used_fallback: bool,
    pub pending: Vec<ProviderChangePreview>,
    pub recognized_keys: Vec<ProviderKey>,
    pub display: ProviderDisplay,
}

#[derive(Clone, Default, PartialEq, Eq)]
pub struct ProviderDisplay {
    pub announcement: Option<String>,
    pub info: Option<ProviderNotice>,
    pub expiry: Option<ProviderNotice>,
    pub support_url: Option<String>,
    pub website_url: Option<String>,
    pub pin: Option<bool>,
    pub keep_expanded: bool,
    pub expand_now: bool,
    pub sort: Option<ProviderSort>,
    pub ping_icon: Option<bool>,
}

impl fmt::Debug for ProviderDisplay {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProviderDisplay")
            .finish_non_exhaustive()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct ProviderNotice {
    pub text: String,
    pub color: ProviderNoticeColor,
    pub button: Option<ProviderNoticeButton>,
}

#[derive(Clone, PartialEq, Eq)]
pub struct ProviderNoticeButton {
    pub label: String,
    pub url: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderNoticeColor {
    Red,
    Blue,
    Green,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderSort {
    Original,
    Latency,
    Alphabetical,
}

#[derive(Debug, Clone)]
pub struct ProfileSummary {
    pub profile_uuid: String,
    pub name: String,
    pub protocol: String,
    pub server: String,
    pub source: Option<Secret>,
    pub subscription_uuid: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileLatency {
    Reachable { milliseconds: u64 },
    TimedOut,
    Unreachable,
    CheckUnreachable,
    Failed(ClientError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileProbeState {
    Queued,
    Running,
    Finished,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProfileProbeStatus {
    pub request_id: RequestId,
    pub method: ProbeMethod,
    pub state: ProfileProbeState,
    pub error: Option<ClientError>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeBatchProgress {
    pub request_id: RequestId,
    pub subscription_uuid: Option<String>,
    pub total: usize,
    pub completed: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProbeCompletion {
    pub request_id: RequestId,
    pub error: Option<ClientError>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientEventKind {
    Started,
    SubscriptionUpdated,
    SubscriptionFallbackUsed,
    SubscriptionFailed(ClientError),
    SubscriptionSettingsSaved,
    SubscriptionSettingsFailed(ClientError),
    SubscriptionRemoved,
    ProfilesImported,
    ProfileUpdated,
    ProfileSelected,
    ProfileRemoved,
    ConnectionStarting,
    Connected(ConnectionMode),
    Disconnected,
    SettingsSaved,
    Failed(ClientError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientEvent {
    pub sequence: u64,
    pub kind: ClientEventKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutingSelection {
    pub profile_uuid: String,
    pub revision: u64,
}

impl From<&RoutingProfile> for RoutingSelection {
    fn from(profile: &RoutingProfile) -> Self {
        Self {
            profile_uuid: profile.uuid.clone(),
            revision: profile.revision,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutingReconnect {
    pub connection_id: String,
    pub target: Option<RoutingSelection>,
}

#[derive(Debug, Clone, Default)]
pub struct RoutingSnapshot {
    pub catalog: RoutingCatalog,
    pub saved: Option<RoutingSelection>,
    pub saved_for_active: Option<RoutingSelection>,
    pub active: Option<RoutingSelection>,
    pub pending_reconnect: Option<RoutingReconnect>,
    pub local_proxy_available: bool,
    pub tun_available: bool,
    pub last_saved_profile_uuid: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ComponentVersions {
    pub expected_blazecore: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TelemetrySettingsSummary {
    pub mode: TelemetryMode,
    pub endpoint: Option<String>,
    pub has_bearer_token: bool,
}

impl From<&TelemetrySettings> for TelemetrySettingsSummary {
    fn from(settings: &TelemetrySettings) -> Self {
        Self {
            mode: settings.mode,
            endpoint: settings.endpoint.clone(),
            has_bearer_token: settings.bearer_token.is_some(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ClientSnapshot {
    pub revision: u64,
    pub completed_request: Option<RequestId>,
    pub subscriptions: Vec<SubscriptionSummary>,
    pub subscription_usage: HashMap<String, SubscriptionUsage>,
    pub provider_subscriptions: Vec<ProviderSubscriptionSummary>,
    pub profiles: Vec<ProfileSummary>,
    pub unavailable_profiles: HashMap<String, Vec<UnavailableProfile>>,
    pub profile_latencies: HashMap<String, ProfileLatency>,
    pub profile_probes: HashMap<String, ProfileProbeStatus>,
    pub probe_batches: Vec<ProbeBatchProgress>,
    pub probe_completions: Vec<ProbeCompletion>,
    pub profile_probe_available: bool,
    pub probe_settings: ProbeSettings,
    pub probe_icmp_available: bool,
    pub selected_profile_uuid: Option<String>,
    pub active_profile_uuid: Option<String>,
    pub connection: ConnectionStatus,
    pub selected_mode: ConnectionMode,
    pub active_mode: Option<ConnectionMode>,
    pub mode_capabilities: ModeCapabilities,
    pub cleanup_required: bool,
    pub local_proxy: Option<String>,
    pub local_proxy_port: u16,
    pub send_subscription_hwid: bool,
    pub subscription_user_agent: Option<Secret>,
    pub subscription_hwid: Option<Secret>,
    pub http_proxy: Option<String>,
    pub busy: bool,
    pub last_error: Option<ClientError>,
    pub language: Language,
    pub component_versions: ComponentVersions,
    pub diagnostics: DiagnosticSummary,
    pub telemetry_export_configured: bool,
    pub telemetry_settings: TelemetrySettingsSummary,
    pub diagnostic_events: Vec<ClientEvent>,
    pub routing: RoutingSnapshot,
}

impl Default for ClientSnapshot {
    fn default() -> Self {
        Self {
            revision: 0,
            completed_request: None,
            subscriptions: Vec::new(),
            subscription_usage: HashMap::new(),
            provider_subscriptions: Vec::new(),
            profiles: Vec::new(),
            unavailable_profiles: HashMap::new(),
            profile_latencies: HashMap::new(),
            profile_probes: HashMap::new(),
            probe_batches: Vec::new(),
            probe_completions: Vec::new(),
            profile_probe_available: false,
            probe_settings: ProbeSettings::default(),
            probe_icmp_available: false,
            selected_profile_uuid: None,
            active_profile_uuid: None,
            connection: ConnectionStatus::Disconnected,
            selected_mode: ConnectionMode::default(),
            active_mode: None,
            mode_capabilities: ModeCapabilities::default(),
            cleanup_required: false,
            local_proxy: None,
            local_proxy_port: 0,
            send_subscription_hwid: true,
            subscription_user_agent: None,
            subscription_hwid: None,
            http_proxy: None,
            busy: true,
            last_error: None,
            language: Language::Russian,
            component_versions: ComponentVersions::default(),
            diagnostics: DiagnosticSummary {
                buffered: 0,
                dropped: 0,
            },
            telemetry_export_configured: false,
            telemetry_settings: TelemetrySettingsSummary::default(),
            diagnostic_events: Vec::new(),
            routing: RoutingSnapshot::default(),
        }
    }
}
