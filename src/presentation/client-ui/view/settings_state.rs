//! Desktop-only preferences and drafts for settings forms.

use vpn_presentation::localization::{self, Key, Locale};
use vpn_presentation::session::{
    ClientSnapshot, DnsPolicy, RequestId, RouteRule, RoutingInput, RoutingProfile,
};

use super::view_model::SettingsPage;

#[derive(Default)]
pub(super) struct DesktopSettings {
    pub interface: InterfaceSettings,
    pub tunnel: TunnelSettings,
    pub advanced: AdvancedSettings,
    pub subscriptions: SubscriptionSettings,
    pub ping: PingSettings,
    pub local_proxy_port_draft: Option<String>,
    pub inbound_auth: [bool; 2],
    pub routing: RoutingSettings,
    pub confirm_reset: bool,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum DesktopTheme {
    #[default]
    System,
    Dark,
    Light,
}

#[derive(Default)]
pub(super) struct InterfaceSettings {
    pub theme: DesktopTheme,
    pub scale_percent: Option<u16>,
}

impl InterfaceSettings {
    pub fn zoom_factor(&self, native_scale: f32) -> f32 {
        self.scale_percent
            .map_or(1.0, |scale| f32::from(scale) / (100.0 * native_scale))
    }
}

#[cfg(test)]
mod interface_tests {
    use super::*;

    #[test]
    fn default_scale_follows_the_monitor_and_manual_scale_is_absolute() {
        let mut interface = InterfaceSettings::default();
        assert_eq!(interface.zoom_factor(1.5), 1.0);
        assert_eq!(interface.zoom_factor(2.0), 1.0);

        interface.scale_percent = Some(150);
        assert_eq!(interface.zoom_factor(1.5), 1.0);
        assert_eq!(interface.zoom_factor(2.0), 0.75);
    }
}

#[derive(Default)]
pub(super) struct TunnelSettings {
    pub fragmentation: bool,
    pub multiplex: bool,
    pub preferred_ip: PreferredIp,
    pub allow_lan: bool,
    pub per_app_mode: PerAppMode,
    pub excluded_apps: Vec<String>,
    pub app_input: String,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum PreferredIp {
    #[default]
    Automatic,
    V4,
    V6,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum PerAppMode {
    #[default]
    System,
    Direct,
    ProxyOnly,
}

#[derive(Default)]
pub(super) struct AdvancedSettings {
    pub local_dns: bool,
    pub json_dns: bool,
    pub resolve_server: bool,
    pub sniffing: bool,
    pub autostart: bool,
    pub bypass_local_proxy: bool,
    pub excluded_routes: Vec<String>,
    pub cidr_input: Option<String>,
    pub invalid_cidr: bool,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum ServerSort {
    #[default]
    Original,
    Ping,
    Alphabetical,
}

pub(super) struct SubscriptionSettings {
    pub user_agent_draft: Option<String>,
    pub auto_update: bool,
    pub interval_hours: u16,
    pub notify_on_update: bool,
    pub timeout_seconds: u16,
    pub update_on_open: bool,
    pub ping_on_open: bool,
    pub connect_on_open: bool,
    pub prevent_duplicates: bool,
    pub collapse: bool,
    pub disable_filter: bool,
    pub server_sort_override: Option<ServerSort>,
}

impl Default for SubscriptionSettings {
    fn default() -> Self {
        Self {
            user_agent_draft: None,
            auto_update: false,
            interval_hours: 24,
            notify_on_update: false,
            timeout_seconds: 30,
            update_on_open: false,
            ping_on_open: false,
            connect_on_open: false,
            prevent_duplicates: true,
            collapse: false,
            disable_filter: false,
            server_sort_override: None,
        }
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum PingMethod {
    #[default]
    Tcp,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum PingDisplay {
    #[default]
    Time,
    Icon,
}

#[derive(Default)]
pub(super) struct PingSettings {
    pub method: PingMethod,
    pub display_override: Option<PingDisplay>,
}

#[derive(Default)]
pub(super) struct RoutingProfileDraft {
    pub profile_uuid: Option<String>,
    pub title: String,
    pub rules: Vec<RouteRule>,
    pub dns_type: DnsType,
    pub dns_endpoint: String,
    pub dns_server_name: String,
    pub dns_path: String,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum DnsType {
    #[default]
    Automatic,
    System,
    Tcp,
    Tls,
    Https,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum RoutingMutation {
    #[default]
    Save,
    Remove,
}

pub(super) enum RoutingDialog {
    ChooseGroup {
        subscription_uuid: Option<String>,
    },
    Title {
        subscription_uuid: Option<String>,
        title: String,
    },
    CopyFromSubscription(Option<String>),
    Remove {
        profile_uuid: String,
        title: String,
    },
}

#[derive(Default)]
pub(super) struct RoutingSettings {
    pub draft: Option<RoutingProfileDraft>,
    pub dialog: Option<RoutingDialog>,
    pub pending: Option<(RequestId, RoutingMutation)>,
    pub invalid_dns_endpoint: bool,
    from_subscription: bool,
}

impl RoutingProfileDraft {
    pub fn from_profile(profile: &RoutingProfile) -> Self {
        Self {
            profile_uuid: Some(profile.uuid.clone()),
            ..Self::from_input(&profile.input)
        }
    }

    pub fn from_input(input: &RoutingInput) -> Self {
        let mut draft = Self {
            title: input.title.clone(),
            rules: input.rules.clone(),
            ..Self::default()
        };
        match &input.dns {
            DnsPolicy::Automatic => {}
            DnsPolicy::System => draft.dns_type = DnsType::System,
            DnsPolicy::Tcp { endpoint } => {
                draft.dns_type = DnsType::Tcp;
                draft.dns_endpoint = endpoint.to_string();
            }
            DnsPolicy::Tls {
                endpoint,
                server_name,
            } => {
                draft.dns_type = DnsType::Tls;
                draft.dns_endpoint = endpoint.to_string();
                draft.dns_server_name.clone_from(server_name);
            }
            DnsPolicy::Https {
                endpoint,
                server_name,
                path,
            } => {
                draft.dns_type = DnsType::Https;
                draft.dns_endpoint = endpoint.to_string();
                draft.dns_server_name.clone_from(server_name);
                draft.dns_path.clone_from(path);
            }
        }
        draft
    }

    pub fn input(&self) -> Option<RoutingInput> {
        Some(RoutingInput {
            title: self.title.trim().to_owned(),
            rules: self.rules.clone(),
            dns: self.dns_policy()?,
        })
    }

    fn dns_policy(&self) -> Option<DnsPolicy> {
        let endpoint = || self.dns_endpoint.trim().parse().ok();
        let server_name = || self.dns_server_name.trim().to_owned();
        match self.dns_type {
            DnsType::Automatic => Some(DnsPolicy::Automatic),
            DnsType::System => Some(DnsPolicy::System),
            DnsType::Tcp => Some(DnsPolicy::Tcp {
                endpoint: endpoint()?,
            }),
            DnsType::Tls => Some(DnsPolicy::Tls {
                endpoint: endpoint()?,
                server_name: server_name(),
            }),
            DnsType::Https => Some(DnsPolicy::Https {
                endpoint: endpoint()?,
                server_name: server_name(),
                path: self.dns_path.trim().to_owned(),
            }),
        }
    }
}

impl RoutingSettings {
    pub fn open_profile(&mut self, draft: Option<RoutingProfileDraft>, from_subscription: bool) {
        self.draft = draft;
        self.from_subscription = from_subscription;
        self.invalid_dns_endpoint = false;
    }

    pub fn profile_parent(&self) -> SettingsPage {
        if self.from_subscription {
            SettingsPage::SubscriptionRouting
        } else {
            SettingsPage::Routing
        }
    }

    pub fn profile_title(&self, locale: Locale) -> &str {
        match &self.draft {
            Some(draft) if !draft.title.trim().is_empty() => &draft.title,
            Some(_) => localization::text(locale, Key::RoutingProfile),
            None => localization::text(locale, Key::RoutingDefaultProfile),
        }
    }

    pub fn acknowledge(&mut self, snapshot: &ClientSnapshot) -> Option<SettingsPage> {
        let (request, mutation) = self.pending?;
        let done = snapshot.completed_request?;
        if done.0 < request.0 {
            return None;
        }
        self.pending = None;
        if done != request || snapshot.last_error.is_some() {
            return None;
        }
        match mutation {
            RoutingMutation::Save => {
                let uuid = snapshot.routing.last_saved_profile_uuid.as_deref()?;
                let profile = snapshot
                    .routing
                    .catalog
                    .profiles
                    .iter()
                    .find(|item| item.uuid == uuid)?;
                self.draft = Some(RoutingProfileDraft::from_profile(profile));
                None
            }
            RoutingMutation::Remove => {
                self.draft = None;
                Some(self.profile_parent())
            }
        }
    }
}
