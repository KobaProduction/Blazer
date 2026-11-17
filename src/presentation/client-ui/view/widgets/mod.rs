//! Native widgets render a borrowed snapshot and return one intent per frame.

mod chrome;
mod connection_controls;
mod controls;
mod dialogs;
mod flags;
mod icons;
mod layout;
mod screens;

use std::time::Instant;

use super::host::UiHost;
#[cfg(all(debug_assertions, feature = "demo"))]
use super::settings_state::RoutingProfileDraft;
use super::settings_state::{DesktopTheme, RoutingMutation};
#[cfg(all(debug_assertions, feature = "demo"))]
use super::view_model::ConfigurationPreview;
use super::view_model::{CatalogOperation, Screen, SettingsPage, ViewState};
use layout::ViewportLayout;
use vpn_presentation::Controller;
use vpn_presentation::localization::{self, Key, Locale};
use vpn_presentation::model::{self, ErrorNotice, Intent};
#[cfg(all(debug_assertions, feature = "demo"))]
use vpn_presentation::session::RequestId;
use vpn_presentation::session::{ClientSnapshot, ConnectionMode};

#[cfg(all(debug_assertions, feature = "demo"))]
pub use controls::selector_capture_id;

pub struct BlazerView {
    controller: Controller,
    state: ViewState,
    title_locale: Option<Locale>,
    interface_zoom: f32,
    platform_error: Option<ErrorNotice>,
    #[cfg(all(debug_assertions, feature = "demo"))]
    demo: bool,
    #[cfg(all(debug_assertions, feature = "demo"))]
    preview_zoom: Option<f32>,
}

impl BlazerView {
    pub fn new(context: &egui::Context, controller: Controller, host: UiHost) -> Self {
        super::fonts::install_emoji_fallback(context);
        super::theme::apply(context);
        Self {
            controller,
            state: ViewState::with_host(host),
            title_locale: None,
            interface_zoom: 1.0,
            platform_error: None,
            #[cfg(all(debug_assertions, feature = "demo"))]
            demo: false,
            #[cfg(all(debug_assertions, feature = "demo"))]
            preview_zoom: None,
        }
    }

    pub fn prefill_import(&mut self, input: String) {
        use super::view_model::ConfigurationKind;
        use vpn_presentation::import::{self, ImportKind};

        let kind = match import::detect(&input) {
            ImportKind::Subscription => ConfigurationKind::Subscription,
            ImportKind::Configuration => ConfigurationKind::Configuration,
        };
        self.state.open_configuration(kind);
        if let Some(draft) = &mut self.state.configuration {
            draft.input = input;
        }
    }

    #[cfg(all(debug_assertions, feature = "demo"))]
    pub fn preview(
        &mut self,
        screen: Screen,
        snapshot: &ClientSnapshot,
        form: Option<ConfigurationPreview>,
    ) {
        self.state.preview = true;
        self.state.open(screen);
        match form {
            Some(ConfigurationPreview::Add(kind)) => self.state.open_configuration(kind),
            Some(ConfigurationPreview::EditSubscription) => {
                if let Some(existing) = snapshot.subscriptions.first() {
                    self.state
                        .open_provider_settings(&existing.subscription_uuid);
                }
            }
            Some(ConfigurationPreview::EditProfile) => {
                if let Some(profile) = snapshot.profiles.first() {
                    self.state.open_profile_editor(&profile.profile_uuid);
                }
            }
            None => {}
        }
        if screen == Screen::Settings(SettingsPage::Subscription)
            && let Some(first) = snapshot.subscriptions.first()
        {
            self.state.open_provider_settings(&first.subscription_uuid);
        }
        if screen == Screen::Settings(SettingsPage::SubscriptionRouting)
            && let Some(first) = snapshot.subscriptions.first()
        {
            self.state
                .open_subscription_routing(&first.subscription_uuid);
        }
        if screen == Screen::Settings(SettingsPage::RoutingProfile)
            && let Some(first) = snapshot.routing.catalog.profiles.first()
        {
            self.state
                .desktop_settings
                .routing
                .open_profile(Some(RoutingProfileDraft::from_profile(first)), false);
        }
    }

    fn update_interface(&mut self, context: &egui::Context) {
        let interface = &self.state.desktop_settings.interface;
        let preference = match interface.theme {
            DesktopTheme::System if self.state.preview => egui::ThemePreference::Dark,
            DesktopTheme::System => egui::ThemePreference::System,
            DesktopTheme::Dark => egui::ThemePreference::Dark,
            DesktopTheme::Light => egui::ThemePreference::Light,
        };
        context.set_theme(preference);
        let native_scale = context.native_pixels_per_point().unwrap_or(1.0);
        let zoom = interface.zoom_factor(native_scale);
        #[cfg(all(debug_assertions, feature = "demo"))]
        let zoom = self.preview_zoom.unwrap_or(zoom);
        if self.interface_zoom != zoom {
            self.interface_zoom = zoom;
            context.set_zoom_factor(zoom);
        }
    }

    #[cfg(all(debug_assertions, feature = "demo"))]
    pub fn enable_demo(&mut self) {
        self.state.preview = true;
        self.demo = true;
    }

    #[cfg(all(debug_assertions, feature = "demo"))]
    pub fn preview_layout(&mut self, expanded: bool, scale_percent: u16) {
        self.state.sidebar_expanded = expanded;
        self.preview_zoom = (scale_percent != 100).then_some(f32::from(scale_percent) / 100.0);
    }

    #[cfg(all(debug_assertions, feature = "demo"))]
    pub fn preview_cidr(&mut self) {
        self.state.desktop_settings.advanced.cidr_input = Some(String::new());
    }

    #[cfg(all(debug_assertions, feature = "demo"))]
    pub fn preview_subscription_refresh(&mut self, uuid: &str, request: RequestId) {
        self.state.catalog_operation = Some((CatalogOperation::Refresh(uuid.to_owned()), request));
    }

    fn handle_intent(&mut self, ctx: &egui::Context, intent: Intent) {
        self.platform_error = None;
        if matches!(intent, Intent::Connect(ConnectionMode::Tun))
            && let Some(start) = &self.state.host.request_tunnel_start
        {
            if !start() {
                self.state.dismissed_error = None;
                self.platform_error = Some(ErrorNotice::Local(Key::EngineFailed));
                ctx.request_repaint();
            }
            return;
        }
        self.submit(intent);
    }

    fn submit(&mut self, intent: Intent) {
        self.state.dismissed_error = None;
        let removal = matches!(intent, Intent::ConfirmRemoval(_));
        let save_telemetry = matches!(intent, Intent::SetTelemetrySettings { .. });
        let save_provider_settings = matches!(
            intent,
            Intent::SaveSubscription {
                subscription_uuid: Some(_),
                ..
            }
        );
        let routing_edit = matches!(
            intent,
            Intent::SaveRoutingProfile { .. }
                | Intent::SetRoutingAssignment { .. }
                | Intent::SetDnsPolicy { .. }
        );
        let configuration = matches!(
            intent,
            Intent::SaveSubscription {
                subscription_uuid: None,
                ..
            } | Intent::ImportProfiles { .. }
        );
        let routing = match &intent {
            Intent::SaveRoutingProfile { .. } => Some(RoutingMutation::Save),
            Intent::RemoveRoutingProfile { .. } => Some(RoutingMutation::Remove),
            _ => None,
        };
        let catalog_operation = match &intent {
            Intent::RefreshSubscription(uuid) => Some(CatalogOperation::Refresh(uuid.clone())),
            Intent::ProbeProfile(uuid) => Some(CatalogOperation::ProbeProfile(uuid.clone())),
            Intent::ProbeProfiles(uuid) => Some(CatalogOperation::ProbeGroup(uuid.clone())),
            _ => None,
        };
        let request = self.controller.submit(intent);
        self.state
            .track_provider_edit(request.filter(|_| save_provider_settings));
        self.state.routing_reconnect.track_edit(
            request.filter(|_| routing_edit),
            self.controller
                .snapshot()
                .routing
                .pending_reconnect
                .as_ref(),
        );
        if save_telemetry && let Some(draft) = &mut self.state.telemetry_settings {
            draft.pending = request;
        }
        if request.is_some() && removal {
            self.state.removal = None;
        }
        if configuration && let Some(draft) = &mut self.state.configuration {
            draft.pending = request;
            draft.feedback = self
                .controller
                .submission_error()
                .map(|error| (self.controller.snapshot().revision, error));
        }
        if let Some(pending) = request.zip(routing) {
            self.state.desktop_settings.routing.pending = Some(pending);
        }
        if let Some(pending) = catalog_operation.zip(request) {
            if matches!(&pending.0, CatalogOperation::Refresh(_)) {
                self.state.refreshed_subscription = None;
            }
            self.state.catalog_operation = Some(pending);
        }
    }

    fn update_title(&mut self, context: &egui::Context) {
        let locale = self.state.locale;
        if self.title_locale == Some(locale) {
            return;
        }
        let title_key = Key::AppName;
        #[cfg(all(debug_assertions, feature = "demo"))]
        let title_key = if self.demo {
            Key::DemoAppName
        } else {
            title_key
        };
        let title = localization::text(locale, title_key).to_owned();
        context.send_viewport_cmd(egui::ViewportCommand::Title(title));
        self.title_locale = Some(locale);
    }
}

impl BlazerView {
    pub fn close(&mut self) {
        if let Err(error) = self.controller.shutdown() {
            eprintln!(
                "{}",
                localization::text(self.state.locale, model::error_key(error))
            );
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui) {
        self.controller.refresh();
        self.state.set_language(self.controller.snapshot().language);
        self.state
            .update_configuration_request(self.controller.snapshot());
        let snapshot = self.controller.snapshot();
        self.state.routing_reconnect.update(snapshot);
        let refresh_error = self.state.update_catalog_request(snapshot, Instant::now());
        if let Some(error) = refresh_error {
            self.state.dismissed_error = Some((snapshot.revision, ErrorNotice::Client(error)));
        }
        self.state.subscription_errors.update(
            &snapshot.diagnostic_events,
            self.state.desktop_settings.subscriptions.notify_on_update,
            refresh_error,
        );
        if let Some(page) = self
            .state
            .desktop_settings
            .routing
            .acknowledge(self.controller.snapshot())
            && self.state.screen == Screen::Settings(SettingsPage::RoutingProfile)
        {
            self.state.open(Screen::Settings(page));
        }
        self.update_title(ui.ctx());
        self.update_interface(ui.ctx());
        let mut view = ViewFrame::new(
            &mut self.state,
            self.controller.snapshot(),
            self.controller.is_saving_mode(),
            self.platform_error.or(self.controller.error()),
        );
        view.render(ui);
        if let Some(intent) = view.intent {
            self.handle_intent(ui.ctx(), intent);
        }
    }
}

struct ViewFrame<'a> {
    view: &'a mut ViewState,
    snapshot: &'a ClientSnapshot,
    saving_mode: bool,
    error: Option<ErrorNotice>,
    intent: Option<Intent>,
    layout: ViewportLayout,
}

impl<'a> ViewFrame<'a> {
    fn new(
        view: &'a mut ViewState,
        snapshot: &'a ClientSnapshot,
        saving_mode: bool,
        error: Option<ErrorNotice>,
    ) -> Self {
        Self {
            view,
            snapshot,
            saving_mode,
            error,
            intent: None,
            layout: ViewportLayout::Desktop,
        }
    }

    fn render(&mut self, ui: &mut egui::Ui) {
        self.layout = ViewportLayout::from_width(ui.available_width());
        self.navigation(ui);
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(ui.visuals().panel_fill))
            .show(ui, |ui| self.workspace(ui));
        self.configuration_dialog(ui);
        self.profile_editor_dialog(ui);
        self.remove_confirmation(ui);
        self.subscription_error_dialog(ui);
        self.routing_reconnect_dialog(ui);
    }

    fn workspace(&mut self, ui: &mut egui::Ui) {
        let screen = self.view.screen;
        if screen == Screen::Home {
            self.screen(ui, screen);
            return;
        }
        let scrolls = screen != Screen::Settings(SettingsPage::Logs);
        let inset = self.layout.content_inset();
        let mut outer_inset = egui::Margin::same(inset);
        let content_inset = if scrolls {
            outer_inset.right = 0;
            egui::Margin {
                right: inset,
                ..egui::Margin::ZERO
            }
        } else {
            egui::Margin::ZERO
        };
        egui::Frame::new().inner_margin(outer_inset).show(ui, |ui| {
            egui::Frame::new()
                .inner_margin(content_inset)
                .show(ui, |ui| self.workspace_header(ui, screen));
            if scrolls {
                ui.spacing_mut().scroll = egui::style::ScrollStyle {
                    content_margin: content_inset,
                    dormant_handle_opacity: 0.45,
                    ..egui::style::ScrollStyle::solid()
                };
                egui::ScrollArea::vertical()
                    .id_salt(screen)
                    .auto_shrink([false, false])
                    .show(ui, |ui| self.screen(ui, screen));
            } else {
                self.screen(ui, screen);
            }
        });
    }

    fn workspace_header(&mut self, ui: &mut egui::Ui, screen: Screen) {
        self.header(ui);
        self.error_banner(ui);
        if let Screen::Settings(
            page @ (SettingsPage::Routing
            | SettingsPage::SubscriptionRouting
            | SettingsPage::RoutingProfile),
        ) = screen
        {
            ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width(), 22.0),
                egui::Layout::right_to_left(egui::Align::Center),
                |ui| self.routing_actions(ui, page),
            );
        }
    }

    fn tr(&self, key: Key) -> &'static str {
        localization::text(self.view.locale, key)
    }

    fn message(&self, key: Key, args: &[(&str, &str)]) -> String {
        localization::format(self.view.locale, key, args)
            .unwrap_or_else(|_| self.tr(key).to_owned())
    }

    fn emit(&mut self, intent: Intent) {
        self.intent.get_or_insert(intent);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resizing_keeps_navigation_and_sidebar_preference() {
        let context = egui::Context::default();
        let snapshot = ClientSnapshot::default();
        let mut state = ViewState::default();
        let screen = Screen::Settings(SettingsPage::Subscription);
        state.navigate(screen);
        state.sidebar_expanded = true;
        state.search = "profile search".into();
        let mut view = ViewFrame::new(&mut state, &snapshot, false, None);

        for (width, layout) in [
            (980.0, ViewportLayout::Desktop),
            (360.0, ViewportLayout::Mobile),
            (760.0, ViewportLayout::Tablet),
            (980.0, ViewportLayout::Desktop),
        ] {
            let mut output = context.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(width, 700.0),
                    )),
                    ..Default::default()
                },
                |ui| view.render(ui),
            );
            output.textures_delta.clear();
            assert_eq!(view.layout, layout);
            assert_eq!(view.view.screen, screen);
            assert!(view.view.sidebar_expanded);
            assert_eq!(view.view.search, "profile search");
        }
    }
}
