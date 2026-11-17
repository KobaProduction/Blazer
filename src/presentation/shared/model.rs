//! Presentation decisions that do not depend on a particular window or toolkit.

use std::borrow::Cow;

use crate::localization::{Key, Locale, text};
use crate::session::{
    ClientError, ClientSnapshot, ConnectionMode, ConnectionStatus, Language, ModeCapabilities,
    ProfileSummary, ProfileUnavailableReason, Secret, TelemetryBearerChange, TelemetryMode,
    UnavailableProfile,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Removal {
    Subscription(String),
    Profile(String),
}

pub enum Intent {
    Connect(ConnectionMode),
    Disconnect,
    ChangeMode(ConnectionMode),
    SetLocalProxyPort(u16),
    SetSubscriptionHwid(bool),
    SetSubscriptionUserAgent(Option<Secret>),
    SetTelemetrySettings {
        mode: TelemetryMode,
        endpoint: String,
        bearer: TelemetryBearerChange,
    },
    ChangeLanguage(Language),
    SelectProfile(String),
    ProbeProfile(String),
    ProbeProfiles(Option<String>),
    SaveRoutingProfile {
        uuid: Option<String>,
        profile: crate::session::RoutingInput,
    },
    RemoveRoutingProfile {
        profile_uuid: String,
    },
    SetRoutingAssignment {
        subscription_uuid: Option<String>,
        assignment: crate::session::RoutingAssignment,
    },
    SetDnsPolicy {
        profile_uuid: String,
        policy: crate::session::DnsPolicy,
    },
    ReconnectRouting(crate::session::RoutingReconnect),
    ImportProfiles {
        title: String,
        text: String,
    },
    SaveSubscription {
        subscription_uuid: Option<String>,
        title: String,
        url: String,
    },
    RefreshSubscription(String),
    SetFallback {
        subscription_uuid: String,
        url: Option<String>,
    },
    ApproveProviderChange {
        subscription_uuid: String,
        nonce: String,
    },
    RejectProviderChange {
        subscription_uuid: String,
        nonce: String,
    },
    ConfirmRemoval(Removal),
}

pub fn unavailable_profile_name(locale: Locale, profile: &UnavailableProfile) -> &str {
    if profile.name.is_empty() {
        text(locale, Key::UnnamedServer)
    } else {
        &profile.name
    }
}

pub fn unavailable_profile_reason(
    locale: Locale,
    reason: ProfileUnavailableReason,
) -> &'static str {
    text(
        locale,
        match reason {
            ProfileUnavailableReason::Unsupported => Key::ProfileUnsupported,
            ProfileUnavailableReason::UnsupportedXhttpUploadMethod => {
                Key::ProfileUnsupportedXhttpUploadMethod
            }
            ProfileUnavailableReason::Invalid => Key::ProfileInvalid,
        },
    )
}

pub fn profile_details_hidden(snapshot: &ClientSnapshot, profile: &ProfileSummary) -> bool {
    profile.subscription_uuid.as_deref().is_some_and(|uuid| {
        snapshot
            .provider_subscriptions
            .iter()
            .any(|provider| provider.subscription_uuid == uuid && provider.raw_config_hidden)
    })
}

pub fn locale(language: Language) -> Locale {
    match language {
        Language::Russian => Locale::Russian,
        Language::English => Locale::English,
    }
}

pub fn mode_key(mode: ConnectionMode) -> Key {
    match mode {
        ConnectionMode::LocalProxy => Key::ModeProxy,
        ConnectionMode::SystemProxy => Key::ModeSystemProxy,
        ConnectionMode::Tun => Key::ModeTun,
    }
}

pub fn mode_available(mode: ConnectionMode, capabilities: ModeCapabilities) -> bool {
    capabilities.supports(mode)
}

pub fn state_key(state: ConnectionStatus, active: Option<ConnectionMode>) -> Key {
    match state {
        ConnectionStatus::Disconnected => Key::Disconnected,
        ConnectionStatus::Connecting => Key::Connecting,
        ConnectionStatus::Connected => match active {
            Some(ConnectionMode::SystemProxy) => Key::SystemProxyRunning,
            Some(ConnectionMode::Tun) => Key::TunRunning,
            _ => Key::ProxyRunning,
        },
        ConnectionStatus::Disconnecting => Key::Disconnecting,
        ConnectionStatus::Failed => Key::Failed,
    }
}

pub fn primary_action(snapshot: &ClientSnapshot, saving_mode: bool) -> (Key, Option<Intent>) {
    use ConnectionStatus::{Connected, Connecting, Disconnecting};
    let stop = Some(Intent::Disconnect);
    match snapshot.connection {
        Connecting => (Key::Cancel, stop),
        Disconnecting => (Key::Disconnecting, None),
        _ if snapshot.cleanup_required => (Key::RetryDisconnect, stop.filter(|_| !snapshot.busy)),
        Connected => {
            let key = match snapshot.active_mode {
                Some(ConnectionMode::LocalProxy) | None => Key::StopProxy,
                _ => Key::Disconnect,
            };
            (key, stop)
        }
        _ => (Key::Connect, connect_intent(snapshot, saving_mode)),
    }
}

fn connect_intent(snapshot: &ClientSnapshot, saving_mode: bool) -> Option<Intent> {
    let mode = snapshot.selected_mode;
    let ready = !snapshot.busy && !saving_mode && snapshot.selected_profile_uuid.is_some();
    (ready && mode_available(mode, snapshot.mode_capabilities)).then_some(Intent::Connect(mode))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorNotice {
    Local(Key),
    Client(ClientError),
}

impl ErrorNotice {
    pub fn key(self) -> Key {
        match self {
            Self::Local(key) => key,
            Self::Client(error) => error_key(error),
        }
    }

    pub fn text(self, locale: Locale) -> Cow<'static, str> {
        match self {
            Self::Client(ClientError::SubscriptionHttp(_)) => {
                self.text_with_code(locale, self.key())
            }
            _ => Cow::Borrowed(text(locale, self.key())),
        }
    }

    pub fn text_with_code(self, locale: Locale, message: Key) -> Cow<'static, str> {
        match self {
            Self::Client(ClientError::SubscriptionHttp(error)) => {
                let status = error.status();
                Cow::Owned(error.detail().map_or_else(
                    || format!("HTTP {status}"),
                    |detail| format!("HTTP {status} — {detail}"),
                ))
            }
            Self::Client(error) => Cow::Owned(format!("{error:?} — {}", text(locale, message))),
            Self::Local(_) => Cow::Borrowed(text(locale, message)),
        }
    }
}

pub fn error_key(error: ClientError) -> Key {
    match error {
        ClientError::InvalidInput | ClientError::InvalidUrl => Key::InvalidInput,
        ClientError::UnsupportedFormat | ClientError::InvalidSubscription => Key::ParseFailed,
        ClientError::UnsupportedFeature | ClientError::UnsupportedMode => Key::Unsupported,
        ClientError::UnsupportedXhttpUploadMethod => Key::ProfileUnsupportedXhttpUploadMethod,
        ClientError::PayloadTooLarge => Key::InputTooLarge,
        ClientError::Network => Key::DownloadFailed,
        ClientError::SubscriptionHttp(_) => Key::SubscriptionHttpFailed,
        ClientError::TelemetryEndpointUnavailable => Key::TelemetryEndpointUnavailable,
        ClientError::Timeout => Key::NetworkTimeout,
        ClientError::Tls => Key::TlsFailed,
        ClientError::StorageUnavailable => Key::StorageUnavailable,
        ClientError::StorageCorrupt => Key::StorageCorrupt,
        ClientError::StorageWrite => Key::StoreFailed,
        ClientError::NoProfileSelected => Key::ProfileRequired,
        ClientError::ProfileNotFound => Key::ProfileMissing,
        ClientError::RoutingProfileMissing => Key::RoutingProfileMissing,
        ClientError::RoutingProfileAssigned => Key::RoutingProfileAssigned,
        ClientError::SubscriptionNotFound => Key::SubscriptionMissing,
        ClientError::EngineNotFound => Key::EngineUnavailable,
        ClientError::EngineInvalid => Key::EngineInvalid,
        ClientError::EngineUntrusted => Key::EngineUntrusted,
        ClientError::EngineStart => Key::EngineFailed,
        ClientError::EngineReadiness => Key::EngineReadiness,
        ClientError::EngineStopped => Key::EngineStopped,
        ClientError::Cancelled => Key::Cancelled,
        ClientError::DisconnectFirst => Key::DisconnectFirst,
        ClientError::ShutdownTimeout => Key::ShutdownTimeout,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hidden_details_apply_only_to_profiles_from_the_provider_subscription() {
        let snapshot = ClientSnapshot {
            provider_subscriptions: vec![crate::session::ProviderSubscriptionSummary {
                subscription_uuid: "hidden".to_owned(),
                raw_config_hidden: true,
                has_committed_fallback: false,
                last_success_unix: None,
                last_update_used_fallback: false,
                pending: Vec::new(),
                recognized_keys: Vec::new(),
                display: Default::default(),
            }],
            ..ClientSnapshot::default()
        };
        let mut profile = ProfileSummary {
            profile_uuid: "server".to_owned(),
            name: "Server".to_owned(),
            protocol: "SOCKS5".to_owned(),
            server: "127.0.0.1:1080".to_owned(),
            subscription_uuid: Some("hidden".to_owned()),
        };
        assert!(profile_details_hidden(&snapshot, &profile));
        profile.subscription_uuid = Some("different".to_owned());
        assert!(!profile_details_hidden(&snapshot, &profile));
        profile.subscription_uuid = None;
        assert!(!profile_details_hidden(&snapshot, &profile));
    }

    #[test]
    fn primary_action_is_truthful_for_pending_and_proxy_states() {
        let mut snapshot = ClientSnapshot {
            busy: false,
            mode_capabilities: ModeCapabilities {
                local_proxy: true,
                ..ModeCapabilities::default()
            },
            ..ClientSnapshot::default()
        };
        assert!(matches!(
            primary_action(&snapshot, false),
            (Key::Connect, None)
        ));
        snapshot.selected_profile_uuid = Some("profile".to_owned());
        assert!(matches!(
            primary_action(&snapshot, false),
            (
                Key::Connect,
                Some(Intent::Connect(ConnectionMode::LocalProxy))
            )
        ));
        snapshot.connection = ConnectionStatus::Connecting;
        snapshot.busy = true;
        assert!(matches!(
            primary_action(&snapshot, false),
            (Key::Cancel, Some(Intent::Disconnect))
        ));
        snapshot.connection = ConnectionStatus::Connected;
        snapshot.busy = false;
        assert!(matches!(
            primary_action(&snapshot, false),
            (Key::StopProxy, Some(Intent::Disconnect))
        ));
        snapshot.connection = ConnectionStatus::Disconnecting;
        assert!(matches!(
            primary_action(&snapshot, false),
            (Key::Disconnecting, None)
        ));
    }

    #[test]
    fn connect_uses_only_an_acknowledged_available_selection() {
        let mut snapshot = ClientSnapshot {
            selected_mode: ConnectionMode::SystemProxy,
            selected_profile_uuid: Some("profile".to_owned()),
            mode_capabilities: ModeCapabilities {
                system_proxy: false,
                ..ModeCapabilities::default()
            },
            busy: false,
            ..ClientSnapshot::default()
        };
        assert!(primary_action(&snapshot, false).1.is_none());
        snapshot.mode_capabilities.system_proxy = true;
        assert!(primary_action(&snapshot, true).1.is_none());
        assert!(matches!(
            primary_action(&snapshot, false).1,
            Some(Intent::Connect(ConnectionMode::SystemProxy))
        ));
    }

    #[test]
    fn active_mode_drives_status_and_cleanup_can_retry_without_profile() {
        assert_eq!(
            state_key(
                ConnectionStatus::Connected,
                Some(ConnectionMode::SystemProxy)
            ),
            Key::SystemProxyRunning
        );
        let snapshot = ClientSnapshot {
            busy: false,
            cleanup_required: true,
            connection: ConnectionStatus::Failed,
            ..ClientSnapshot::default()
        };
        assert!(matches!(
            primary_action(&snapshot, false),
            (Key::RetryDisconnect, Some(Intent::Disconnect))
        ));
    }
}
