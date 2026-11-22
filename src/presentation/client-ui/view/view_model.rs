use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use vpn_presentation::localization::{Key, Locale};
use vpn_presentation::model::{ErrorNotice, Removal, locale};
use vpn_presentation::session::{
    ClientError, ClientEvent, ClientEventKind, ClientSnapshot, Language, ProfileSummary, RequestId,
    RoutingReconnect, TelemetryMode, TelemetrySettingsSummary,
};

use super::import_source::ImportSource;
use super::settings_state::DesktopSettings;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Screen {
    Home,
    Statistics,
    Settings(SettingsPage),
}

impl Screen {
    pub fn key(self) -> Key {
        match self {
            Self::Home => Key::Servers,
            Self::Statistics => Key::Statistics,
            Self::Settings(page) => page.key(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SettingsPage {
    Index,
    Routing,
    RoutingProfile,
    SubscriptionRouting,
    Advanced,
    Inbounds,
    ExcludedRoutes,
    UrlSchemes,
    PerAppProxy,
    Subscriptions,
    Subscription,
    Ping,
    Logs,
    Telemetry,
    About,
}

impl SettingsPage {
    pub fn key(self) -> Key {
        match self {
            Self::Index => Key::Settings,
            Self::Routing => Key::RoutingRules,
            Self::RoutingProfile => Key::RoutingProfile,
            Self::SubscriptionRouting => Key::Routing,
            Self::Advanced => Key::AdvancedSettings,
            Self::Inbounds => Key::Inbounds,
            Self::ExcludedRoutes => Key::ExcludedRoutes,
            Self::UrlSchemes => Key::UrlSchemes,
            Self::PerAppProxy => Key::PerAppProxy,
            Self::Subscriptions => Key::Subscriptions,
            Self::Subscription => Key::SubscriptionSettings,
            Self::Ping => Key::Ping,
            Self::Logs => Key::Logs,
            Self::Telemetry => Key::OpenTelemetry,
            Self::About => Key::About,
        }
    }
}

pub(super) struct ProviderSettingsDraft {
    pub subscription_uuid: String,
    pub title: Option<String>,
    pub url: Option<String>,
    pub url_visible: bool,
    pub fallback_url: String,
    pub pending: Option<RequestId>,
}

pub(super) struct TelemetrySettingsDraft {
    saved: TelemetrySettingsSummary,
    pub mode: TelemetryMode,
    pub endpoint: String,
    pub bearer: String,
    pub clear_bearer: bool,
    pub pending: Option<RequestId>,
    pub outcome: Option<Key>,
}

impl TelemetrySettingsDraft {
    pub fn new(saved: &TelemetrySettingsSummary) -> Self {
        Self {
            saved: saved.clone(),
            mode: match saved.mode {
                TelemetryMode::Manual => TelemetryMode::Manual,
                _ => TelemetryMode::Disabled,
            },
            endpoint: saved.endpoint.clone().unwrap_or_default(),
            bearer: String::new(),
            clear_bearer: false,
            pending: None,
            outcome: None,
        }
    }

    pub fn synchronize(&mut self, snapshot: &ClientSnapshot) {
        let completed = self.pending.zip(snapshot.completed_request);
        let acknowledged = completed.is_some_and(|(pending, done)| pending.0 <= done.0);
        let outcome = completed
            .filter(|(pending, done)| pending == done)
            .map(|_| {
                snapshot.last_error.map_or_else(
                    || match snapshot.telemetry_settings.mode {
                        TelemetryMode::Manual => Key::TelemetryCheckPassed,
                        _ => Key::TelemetryDisabled,
                    },
                    vpn_presentation::model::error_key,
                )
            });
        if self.saved != snapshot.telemetry_settings
            || (acknowledged && snapshot.last_error.is_none())
        {
            *self = Self::new(&snapshot.telemetry_settings);
        } else if acknowledged {
            self.pending = None;
        }
        if let Some(outcome) = outcome {
            self.outcome = Some(outcome);
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ConfigurationKind {
    #[default]
    Subscription,
    Configuration,
    Clipboard,
    Qr,
}

#[cfg(all(debug_assertions, feature = "demo"))]
pub enum ConfigurationPreview {
    Add(ConfigurationKind),
    EditSubscription,
    EditProfile,
}

impl ConfigurationKind {
    pub(super) fn key(self) -> Key {
        match self {
            Self::Subscription => Key::AddSubscription,
            Self::Configuration => Key::AddConfiguration,
            Self::Clipboard => Key::AddClipboard,
            Self::Qr => Key::AddQr,
        }
    }

    pub(super) fn import_kind(self) -> Option<vpn_presentation::import::ImportKind> {
        use vpn_presentation::import::ImportKind;
        match self {
            Self::Subscription => Some(ImportKind::Subscription),
            Self::Configuration => Some(ImportKind::Configuration),
            Self::Qr | Self::Clipboard => None,
        }
    }
}

#[derive(Default)]
pub(super) struct ConfigurationDraft {
    pub kind: ConfigurationKind,
    pub title: String,
    pub input: String,
    pub pending: Option<RequestId>,
    pub feedback: Option<(u64, ErrorNotice)>,
    pub source: ImportSource,
}

#[derive(PartialEq, Eq)]
pub(super) enum ProbeScope {
    Group(Option<String>),
    Profile(String),
}

#[derive(Default)]
pub(super) struct RoutingReconnectPrompt {
    edit: Option<(RequestId, Option<RoutingReconnect>)>,
    pub confirmation: Option<RoutingReconnect>,
}

impl RoutingReconnectPrompt {
    pub fn awaiting_edit(&self) -> bool {
        self.edit.is_some()
    }

    pub fn track_edit(&mut self, request: Option<RequestId>, before: Option<&RoutingReconnect>) {
        if let Some(request) = request {
            self.edit = Some((request, before.cloned()));
        }
    }

    pub fn update(&mut self, snapshot: &ClientSnapshot) {
        let current = snapshot.routing.pending_reconnect.as_ref();
        if self.confirmation.as_ref() != current {
            self.confirmation = None;
        }
        let Some(((request, before), completed)) =
            self.edit.as_ref().zip(snapshot.completed_request)
        else {
            return;
        };
        if completed.0 < request.0 {
            return;
        }
        if completed == *request && snapshot.last_error.is_none() && current != before.as_ref() {
            self.confirmation = current.cloned();
        }
        self.edit = None;
    }
}

const SUBSCRIPTION_NOTICE_LIMIT: usize = 4;
const REFRESH_SUCCESS_DURATION: Duration = Duration::from_secs(3);

#[derive(Default)]
pub(super) struct SubscriptionErrors {
    last_sequence: u64,
    pending: VecDeque<ClientError>,
}

impl SubscriptionErrors {
    pub fn update(
        &mut self,
        events: &[ClientEvent],
        background_enabled: bool,
        manual_error: Option<ClientError>,
    ) {
        let mut duplicate = manual_error;
        let last_sequence = self.last_sequence;
        for event in events.iter().filter(|event| event.sequence > last_sequence) {
            self.last_sequence = self.last_sequence.max(event.sequence);
            if let ClientEventKind::SubscriptionFailed(error) = event.kind {
                if duplicate == Some(error) {
                    duplicate = None;
                } else if background_enabled {
                    self.push(error);
                }
            }
        }
        if let Some(error) = manual_error {
            self.push(error);
        }
    }

    fn push(&mut self, error: ClientError) {
        if self.pending.len() == SUBSCRIPTION_NOTICE_LIMIT {
            self.pending.pop_back();
        }
        self.pending.push_back(error);
    }

    pub fn current(&self) -> Option<ClientError> {
        self.pending.front().copied()
    }

    pub fn dismiss(&mut self) {
        self.pending.pop_front();
    }
}

pub(super) struct ProfileEditor {
    pub uuid: String,
    pub text: String,
    pub pending: Option<RequestId>,
    pub feedback: Option<(u64, ErrorNotice)>,
}

pub struct ViewState {
    pub(super) host: super::host::UiHost,
    pub screen: Screen,
    back_stack: Vec<Screen>,
    pub locale: Locale,
    pub sidebar_expanded: bool,
    pub manual_group_collapsed: bool,
    pub collapsed_subscriptions: std::collections::HashSet<String>,
    pub pinned_subscriptions: HashMap<String, bool>,
    provider_expand_updates: HashMap<String, u64>,
    pub search: String,
    pub removal: Option<Removal>,
    pub(super) configuration: Option<ConfigurationDraft>,
    pub(super) profile_editor: Option<ProfileEditor>,
    pub(super) subscription_refresh: Option<(String, RequestId)>,
    pub(super) probe: Option<(ProbeScope, RequestId)>,
    pub(super) routing_reconnect: RoutingReconnectPrompt,
    pub(super) refreshed_subscription: Option<(String, Instant)>,
    pub(super) subscription_errors: SubscriptionErrors,
    pub(super) dismissed_error: Option<(u64, ErrorNotice)>,
    pub(super) desktop_settings: DesktopSettings,
    pub subscription_routing_uuid: Option<String>,
    pub(super) preview: bool,
    pub(super) provider_settings: Option<ProviderSettingsDraft>,
    pub(super) telemetry_settings: Option<TelemetrySettingsDraft>,
}

impl Default for ViewState {
    fn default() -> Self {
        Self {
            host: super::host::UiHost::default(),
            screen: Screen::Home,
            back_stack: Vec::new(),
            locale: Locale::Russian,
            sidebar_expanded: false,
            manual_group_collapsed: false,
            collapsed_subscriptions: Default::default(),
            pinned_subscriptions: Default::default(),
            provider_expand_updates: HashMap::new(),
            search: String::new(),
            removal: None,
            configuration: None,
            profile_editor: None,
            subscription_refresh: None,
            probe: None,
            routing_reconnect: RoutingReconnectPrompt::default(),
            refreshed_subscription: None,
            subscription_errors: SubscriptionErrors::default(),
            dismissed_error: None,
            desktop_settings: DesktopSettings::default(),
            subscription_routing_uuid: None,
            preview: false,
            provider_settings: None,
            telemetry_settings: None,
        }
    }
}

impl ViewState {
    pub(super) fn with_host(host: super::host::UiHost) -> Self {
        Self {
            host,
            ..Self::default()
        }
    }

    pub fn navigate(&mut self, screen: Screen) {
        if screen != self.screen {
            self.telemetry_settings = None;
            if self.screen == Screen::Settings(SettingsPage::Subscription) {
                self.provider_settings = None;
            }
        }
        self.back_stack.clear();
        if screen == Screen::Settings(SettingsPage::Index) {
            self.back_stack.push(Screen::Home);
        }
        self.screen = screen;
    }

    pub fn open(&mut self, screen: Screen) {
        if screen == self.screen {
            return;
        }
        self.telemetry_settings = None;
        if self.screen == Screen::Settings(SettingsPage::Subscription) {
            self.provider_settings = None;
        }
        if let Some(previous) = self.back_stack.iter().position(|page| *page == screen) {
            self.back_stack.truncate(previous);
        } else {
            self.back_stack.push(self.screen);
        }
        self.screen = screen;
    }

    pub(super) fn previous(&self) -> Option<Screen> {
        self.back_stack.last().copied()
    }

    pub fn set_language(&mut self, language: Language) {
        self.locale = locale(language);
    }

    pub fn open_configuration(&mut self, kind: ConfigurationKind) {
        self.configuration = Some(ConfigurationDraft {
            kind,
            ..Default::default()
        });
    }

    pub(super) fn update_configuration_request(&mut self, snapshot: &ClientSnapshot) {
        let Some(draft) = &mut self.configuration else {
            return;
        };
        let Some((pending, done)) = draft.pending.zip(snapshot.completed_request) else {
            return;
        };
        if pending.0 > done.0 {
            return;
        }

        draft.pending = None;
        if pending != done {
            return;
        }
        if let Some(error) = snapshot.last_error {
            draft.feedback = Some((snapshot.revision, ErrorNotice::Client(error)));
        } else {
            self.configuration = None;
            self.search.clear();
            self.navigate(Screen::Home);
        }
    }

    pub(super) fn update_catalog_request(
        &mut self,
        snapshot: &ClientSnapshot,
        now: Instant,
    ) -> Option<ClientError> {
        self.apply_provider_expansion(snapshot);
        if self
            .refreshed_subscription
            .as_ref()
            .is_some_and(|(_, deadline)| *deadline <= now)
        {
            self.refreshed_subscription = None;
        }
        let done = snapshot.completed_request?;
        if self
            .probe
            .as_ref()
            .is_some_and(|(_, pending)| pending.0 <= done.0)
        {
            self.probe = None;
        }
        if self.subscription_refresh.as_ref()?.1.0 > done.0 {
            return None;
        }
        let (uuid, pending) = self.subscription_refresh.take()?;
        if pending != done {
            return None;
        }
        self.refreshed_subscription = snapshot
            .last_error
            .is_none()
            .then_some((uuid, now + REFRESH_SUCCESS_DURATION));
        snapshot.last_error
    }

    fn apply_provider_expansion(&mut self, snapshot: &ClientSnapshot) {
        self.provider_expand_updates.retain(|uuid, _| {
            snapshot
                .provider_subscriptions
                .iter()
                .any(|provider| &provider.subscription_uuid == uuid && provider.display.expand_now)
        });
        let refreshed = match (&self.subscription_refresh, snapshot.completed_request) {
            (Some((uuid, pending)), Some(done))
                if *pending == done && snapshot.last_error.is_none() =>
            {
                Some(uuid.as_str())
            }
            _ => None,
        };
        for provider in &snapshot.provider_subscriptions {
            let Some(updated) = provider
                .last_success_unix
                .filter(|_| provider.display.expand_now)
            else {
                continue;
            };
            let uuid = &provider.subscription_uuid;
            if self.provider_expand_updates.get(uuid) != Some(&updated)
                || refreshed == Some(uuid.as_str())
            {
                self.provider_expand_updates.insert(uuid.clone(), updated);
                self.collapsed_subscriptions.remove(uuid);
            }
        }
    }

    pub(super) fn open_profile_editor(&mut self, profile: &ProfileSummary) {
        self.profile_editor = Some(ProfileEditor {
            uuid: profile.profile_uuid.clone(),
            text: profile
                .source
                .as_ref()
                .map_or_else(String::new, |source| source.expose().to_owned()),
            pending: None,
            feedback: None,
        });
    }

    pub(super) fn update_profile_request(&mut self, snapshot: &ClientSnapshot) {
        let Some(draft) = &mut self.profile_editor else {
            return;
        };
        let Some((pending, done)) = draft.pending.zip(snapshot.completed_request) else {
            return;
        };
        if pending.0 > done.0 {
            return;
        }
        draft.pending = None;
        if pending != done {
            return;
        }
        if let Some(error) = snapshot.last_error {
            draft.feedback = Some((snapshot.revision, ErrorNotice::Client(error)));
        } else {
            self.profile_editor = None;
        }
    }

    pub fn open_subscription_routing(&mut self, subscription_uuid: &str) {
        self.subscription_routing_uuid = Some(subscription_uuid.to_owned());
        self.open(Screen::Settings(SettingsPage::SubscriptionRouting));
    }

    pub(super) fn open_provider_settings(&mut self, subscription_uuid: &str) {
        if self
            .provider_settings
            .as_ref()
            .is_none_or(|draft| draft.subscription_uuid != subscription_uuid)
        {
            self.provider_settings = Some(ProviderSettingsDraft {
                subscription_uuid: subscription_uuid.to_owned(),
                title: None,
                url: None,
                url_visible: false,
                fallback_url: String::new(),
                pending: None,
            });
        }
        self.open(Screen::Settings(SettingsPage::Subscription));
    }

    pub(super) fn track_provider_edit(&mut self, request: Option<RequestId>) {
        if let (Some(draft), Some(request)) = (&mut self.provider_settings, request) {
            draft.pending = Some(request);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_editor_prefills_the_source_and_retains_edits_after_failure() {
        let profile = ProfileSummary {
            profile_uuid: "profile".into(),
            name: "Server".into(),
            protocol: "SOCKS5".into(),
            server: "127.0.0.1:1080".into(),
            source: Some(vpn_presentation::session::Secret::new(
                "socks5://127.0.0.1:1080",
            )),
            subscription_uuid: None,
        };
        let mut state = ViewState::default();
        state.open_profile_editor(&profile);
        let draft = state.profile_editor.as_mut().unwrap();
        assert_eq!(draft.text, "socks5://127.0.0.1:1080");
        draft.text = "edited text".into();
        draft.pending = Some(RequestId(1));
        let mut snapshot = ClientSnapshot {
            completed_request: Some(RequestId(1)),
            last_error: Some(ClientError::InvalidSubscription),
            ..Default::default()
        };
        state.update_profile_request(&snapshot);
        let draft = state.profile_editor.as_mut().unwrap();
        assert_eq!(draft.text, "edited text");
        assert!(draft.feedback.is_some());
        draft.pending = Some(RequestId(2));
        snapshot.completed_request = Some(RequestId(2));
        snapshot.last_error = None;
        state.update_profile_request(&snapshot);
        assert!(state.profile_editor.is_none());
    }

    #[test]
    fn routing_reconnect_prompt_requires_a_successful_edit_to_the_effective_route() {
        use vpn_presentation::session::RoutingSelection;

        let mut prompt = RoutingReconnectPrompt::default();
        let mut snapshot = ClientSnapshot::default();
        let pending = RoutingReconnect {
            connection_id: "current-connection".into(),
            target: Some(RoutingSelection {
                profile_uuid: "assigned-profile".into(),
                revision: 2,
            }),
        };
        let request = RequestId(1);
        prompt.track_edit(Some(request), None);
        snapshot.routing.pending_reconnect = Some(pending.clone());
        prompt.update(&snapshot);
        assert!(prompt.confirmation.is_none());
        snapshot.completed_request = Some(request);
        prompt.update(&snapshot);
        assert_eq!(prompt.confirmation.take(), Some(pending.clone()));
        prompt.update(&snapshot);
        assert!(prompt.confirmation.is_none());

        let unchanged = RequestId(2);
        prompt.track_edit(Some(unchanged), Some(&pending));
        snapshot.completed_request = Some(unchanged);
        prompt.update(&snapshot);
        assert!(prompt.confirmation.is_none());

        let failed = RequestId(3);
        prompt.track_edit(Some(failed), Some(&pending));
        snapshot.completed_request = Some(failed);
        snapshot.last_error = Some(ClientError::InvalidInput);
        snapshot.routing.pending_reconnect = Some(RoutingReconnect {
            target: None,
            ..pending
        });
        prompt.update(&snapshot);
        assert!(prompt.confirmation.is_none());
        assert!(!prompt.awaiting_edit());

        prompt.confirmation = snapshot.routing.pending_reconnect.take();
        prompt.update(&snapshot);
        assert!(prompt.confirmation.is_none());
    }

    #[test]
    fn subscription_error_dialog_ignores_success_and_deduplicates_manual_failure() {
        let mut errors = SubscriptionErrors::default();
        let events = [
            ClientEvent {
                sequence: 1,
                kind: ClientEventKind::SubscriptionUpdated,
            },
            ClientEvent {
                sequence: 2,
                kind: ClientEventKind::SubscriptionFallbackUsed,
            },
            ClientEvent {
                sequence: 3,
                kind: ClientEventKind::SubscriptionFailed(ClientError::Network),
            },
        ];
        errors.update(&events[..2], true, None);
        assert_eq!(errors.current(), None);
        errors.update(&events, true, Some(ClientError::Network));
        assert_eq!(errors.current(), Some(ClientError::Network));
        errors.dismiss();
        errors.update(&events, true, None);
        assert_eq!(errors.current(), None);

        let mut errors = SubscriptionErrors::default();
        errors.update(&events, false, None);
        errors.update(&events, true, None);
        assert_eq!(errors.current(), None);
        errors.update(&events, false, Some(ClientError::Network));
        assert_eq!(errors.current(), Some(ClientError::Network));
    }

    #[test]
    fn subscription_refresh_feedback_follows_the_matching_request() {
        let mut state = ViewState::default();
        let now = Instant::now();
        let request = RequestId(2);
        state.subscription_refresh = Some(("sub".into(), request));
        let mut snapshot = ClientSnapshot {
            completed_request: Some(RequestId(1)),
            ..ClientSnapshot::default()
        };
        assert_eq!(state.update_catalog_request(&snapshot, now), None);
        assert!(state.subscription_refresh.is_some());
        assert_eq!(state.refreshed_subscription, None);

        snapshot.completed_request = Some(request);
        assert_eq!(state.update_catalog_request(&snapshot, now), None);
        assert!(state.subscription_refresh.is_none());
        assert_eq!(
            state.refreshed_subscription,
            Some(("sub".into(), now + REFRESH_SUCCESS_DURATION))
        );

        state.subscription_refresh = Some(("sub".into(), RequestId(3)));
        snapshot.completed_request = Some(RequestId(3));
        snapshot.last_error = Some(ClientError::Network);
        assert_eq!(
            state.update_catalog_request(&snapshot, now),
            Some(ClientError::Network)
        );
        assert_eq!(state.refreshed_subscription, None);
    }

    #[test]
    fn probe_completion_leaves_a_queued_subscription_refresh_pending() {
        let mut state = ViewState::default();
        let now = Instant::now();
        state.probe = Some((ProbeScope::Profile("server".into()), RequestId(1)));
        state.subscription_refresh = Some(("sub".into(), RequestId(2)));
        let snapshot = ClientSnapshot {
            completed_request: Some(RequestId(1)),
            ..ClientSnapshot::default()
        };
        assert_eq!(state.update_catalog_request(&snapshot, now), None);
        assert!(state.probe.is_none());
        assert_eq!(
            state.subscription_refresh,
            Some(("sub".into(), RequestId(2)))
        );
        assert_eq!(state.refreshed_subscription, None);
    }

    #[test]
    fn breadcrumbs_trim_navigation_history() {
        use SettingsPage::{Advanced, Index, Subscriptions};
        let mut state = ViewState::default();
        for page in [Index, Subscriptions, Advanced] {
            state.open(Screen::Settings(page));
        }
        state.open(Screen::Settings(Subscriptions));
        assert_eq!(state.screen, Screen::Settings(Subscriptions));
        state.open(Screen::Settings(Index));
        assert_eq!(state.previous(), Some(Screen::Home));
    }

    #[test]
    fn configuration_draft_closes_only_after_acknowledged_success() {
        use vpn_presentation::session::ClientError;

        let mut state = ViewState::default();
        state.open_configuration(ConfigurationKind::Subscription);
        let draft = state.configuration.as_mut().unwrap();
        draft.title = "Updated title".to_owned();
        draft.pending = Some(RequestId(2));
        let mut snapshot = ClientSnapshot {
            completed_request: Some(RequestId(1)),
            ..Default::default()
        };
        state.update_configuration_request(&snapshot);
        assert_eq!(
            state.configuration.as_ref().unwrap().pending,
            Some(RequestId(2))
        );

        snapshot.completed_request = Some(RequestId(2));
        snapshot.last_error = Some(ClientError::InvalidSubscription);
        state.update_configuration_request(&snapshot);
        let draft = state.configuration.as_mut().unwrap();
        assert_eq!(draft.title, "Updated title");
        assert_eq!(draft.pending, None);
        assert_eq!(
            draft.feedback,
            Some((
                snapshot.revision,
                ErrorNotice::Client(ClientError::InvalidSubscription)
            ))
        );

        draft.pending = Some(RequestId(3));
        snapshot.completed_request = Some(RequestId(3));
        snapshot.last_error = None;
        state.update_configuration_request(&snapshot);
        assert!(state.configuration.is_none());
    }

    #[test]
    fn language_changes_preserve_drafts_screen_and_identity() {
        let mut state = ViewState {
            screen: Screen::Settings(SettingsPage::SubscriptionRouting),
            subscription_routing_uuid: Some("stable-uuid".to_owned()),
            configuration: Some(ConfigurationDraft {
                title: "Name".to_owned(),
                input: "https://provider.example/private".to_owned(),
                ..Default::default()
            }),
            ..Default::default()
        };
        state.set_language(Language::English);
        assert_eq!(state.locale, Locale::English);
        assert_eq!(
            state.screen,
            Screen::Settings(SettingsPage::SubscriptionRouting)
        );
        let draft = state.configuration.expect("configuration draft");
        assert_eq!(draft.title, "Name");
        assert_eq!(draft.input, "https://provider.example/private");
        assert_eq!(
            state.subscription_routing_uuid.as_deref(),
            Some("stable-uuid")
        );
    }
}
