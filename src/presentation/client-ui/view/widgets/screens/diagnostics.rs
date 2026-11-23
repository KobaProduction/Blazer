mod about;

use std::fmt::Write as _;

use vpn_presentation::APP_VERSION;
use vpn_presentation::localization::Key;
use vpn_presentation::model::{ErrorNotice, error_key, state_key};
use vpn_presentation::session::{ClientError, ClientEventKind, ConnectionStatus};

use super::ViewFrame;
use super::settings::{divider, group, setting_row};
use crate::AppUpdateStatus;
use crate::view::widgets::connection_controls::CONNECTION_CONTROL_WIDTH;
use crate::view::widgets::controls::{action_button, more_button};
use crate::view::widgets::layout::modal_content_width;

const MISSING: &str = "—";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
enum LogTab {
    #[default]
    Application,
    Core,
}

impl LogTab {
    const ALL: [Self; 2] = [Self::Application, Self::Core];

    fn key(self) -> Key {
        match self {
            Self::Application => Key::LogCategoryApplication,
            Self::Core => Key::LogCore,
        }
    }

    fn source(event: ClientEventKind) -> Self {
        match event {
            ClientEventKind::ConnectionStarting
            | ClientEventKind::Connected(_)
            | ClientEventKind::Disconnected
            | ClientEventKind::Failed(
                ClientError::EngineNotFound
                | ClientError::EngineUntrusted
                | ClientError::EngineInvalid
                | ClientError::EngineStart
                | ClientError::EngineReadiness
                | ClientError::EngineStopped
                | ClientError::TunConflict(_)
                | ClientError::TunPreflightUnavailable
                | ClientError::TunServiceUnavailable
                | ClientError::LinuxTunUnavailable
                | ClientError::ShutdownTimeout,
            ) => Self::Core,
            _ => Self::Application,
        }
    }
}

impl ViewFrame<'_> {
    pub(super) fn statistics(&self, ui: &mut egui::Ui) {
        group(ui, Some(self.tr(Key::StatsServer)), |ui| {
            setting_row(ui, self.tr(Key::StatsConnection), |ui| {
                ui.add(egui::Label::new(self.connection_summary()).truncate());
            });
            ui.separator();
            self.unreported_row(ui, Key::StatsStartedAt);
            ui.separator();
            self.unreported_row(ui, Key::StatsDuration);
        });
        for title in [
            Key::StatsProxyBandwidth,
            Key::StatsProxyUsage,
            Key::StatsDirectUsage,
        ] {
            group(ui, Some(self.tr(title)), |ui| {
                self.unreported_row(ui, Key::StatsDownload);
                ui.separator();
                self.unreported_row(ui, Key::StatsUpload);
            });
        }
    }

    fn connection_summary(&self) -> &str {
        let snapshot = self.snapshot;
        if snapshot.connection == ConnectionStatus::Disconnected {
            return self.tr(Key::StatsNotConnected);
        }
        if snapshot.connection == ConnectionStatus::Connected
            && let Some(active_uuid) = &snapshot.active_profile_uuid
            && let Some(profile) = snapshot
                .profiles
                .iter()
                .find(|profile| &profile.profile_uuid == active_uuid)
        {
            return &profile.name;
        }
        self.tr(state_key(snapshot.connection, snapshot.active_mode))
    }

    fn unreported_row(&self, ui: &mut egui::Ui, label: Key) {
        setting_row(ui, self.tr(label), |ui| self.unreported_value(ui));
    }

    fn unreported_value(&self, ui: &mut egui::Ui) {
        ui.weak(MISSING)
            .on_hover_text(self.tr(Key::StatsUnavailable));
    }

    pub(super) fn logs(&self, ui: &mut egui::Ui) {
        let tab_id = egui::Id::unique("diagnostics-log-tab");
        let report_id = egui::Id::unique("diagnostics-report");
        let mut selected = ui
            .ctx()
            .data_mut(|data| data.get_temp::<LogTab>(tab_id))
            .unwrap_or_default();
        let mut report_open = ui
            .ctx()
            .data_mut(|data| data.get_temp::<bool>(report_id))
            .unwrap_or_default();
        report_open |= ui.input_mut(|input| input.consume_key(egui::Modifiers::CTRL, egui::Key::R));
        self.log_toolbar(ui, &mut selected, &mut report_open);
        ui.add_space(6.0);
        self.log_viewer(ui, selected);

        if report_open {
            report_open = !self.report_dialog(ui, report_id);
        }
        ui.ctx().data_mut(|data| {
            data.insert_temp(tab_id, selected);
            data.insert_temp(report_id, report_open);
        });
    }

    fn log_toolbar(&self, ui: &mut egui::Ui, selected: &mut LogTab, report_open: &mut bool) {
        ui.horizontal(|ui| {
            let tabs_width = (ui.available_width() - 36.0 - ui.spacing().item_spacing.x).max(0.0);
            egui::ScrollArea::horizontal()
                .id_salt("diagnostics-log-tabs")
                .max_width(tabs_width)
                .min_scrolled_width(0.0)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        for tab in LogTab::ALL {
                            self.log_tab(ui, selected, tab);
                        }
                    });
                });
            let label = self.tr(Key::LogActions);
            let (response, _) = egui::containers::menu::MenuButton::from_button(more_button(32.0))
                .ui(ui, |ui| {
                    let log = self.log_text(Some(*selected));
                    let has_events = !log.is_empty();
                    if ui
                        .add_enabled(has_events, egui::Button::new(self.tr(Key::LogCopy)))
                        .on_disabled_hover_text(self.tr(Key::LogEmpty))
                        .clicked()
                    {
                        ui.ctx().copy_text(log);
                        ui.close();
                    }
                    if ui.button(self.tr(Key::LogReport)).clicked() {
                        *report_open = true;
                        ui.close();
                    }
                    ui.separator();
                    ui.label(self.tr(Key::DiagnosticsPrivacy));
                });
            response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
            response.on_hover_text(label);
        });
    }

    fn log_tab(&self, ui: &mut egui::Ui, selected: &mut LogTab, tab: LogTab) {
        let active = *selected == tab;
        let button = egui::Button::new(self.tr(tab.key()))
            .frame_when_inactive(false)
            .min_size(egui::vec2(0.0, 32.0));
        let response = ui.add(button);
        if response.clicked() {
            *selected = tab;
        }
        if active {
            ui.painter().hline(
                response.rect.x_range(),
                response.rect.bottom(),
                (2.0, ui.visuals().hyperlink_color),
            );
        }
    }

    fn log_viewer(&self, ui: &mut egui::Ui, tab: LogTab) {
        egui::Frame::new()
            .fill(ui.visuals().panel_fill)
            .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
            .corner_radius(4)
            .inner_margin(12)
            .show(ui, |ui| {
                egui::ScrollArea::both()
                    .id_salt(("diagnostics-log-content", tab))
                    .min_scrolled_height(0.0)
                    .min_scrolled_width(0.0)
                    .auto_shrink([false, false])
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        let text = self.log_text(Some(tab));
                        if text.is_empty() {
                            ui.weak(self.tr(Key::LogEmpty));
                        } else {
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(text)
                                        .monospace()
                                        .size(12.0)
                                        .color(ui.visuals().strong_text_color()),
                                )
                                .extend()
                                .selectable(true),
                            );
                        }
                    });
            });
    }

    fn log_text(&self, tab: Option<LogTab>) -> String {
        let mut text = String::new();
        for event in &self.snapshot.diagnostic_events {
            if tab.is_some_and(|tab| tab != LogTab::source(event.kind)) {
                continue;
            }
            let (category, message) = event_labels(event.kind);
            let sequence = event.sequence;
            let category = self.tr(category);
            let message = match event.kind {
                ClientEventKind::Failed(error @ ClientError::TunConflict(_)) => {
                    ErrorNotice::Client(error).text(self.view.locale)
                }
                _ => self.tr(message).into(),
            };
            if self.view.preview {
                let seconds = sequence.min(599);
                let _ = write!(
                    text,
                    "2026-11-11  12:{:02}:{:02}  ",
                    seconds / 60,
                    seconds % 60
                );
            }
            let _ = writeln!(text, "#{sequence:03}  [{category}]  {message}");
        }
        text
    }

    fn report_dialog(&self, ui: &egui::Ui, id: egui::Id) -> bool {
        egui::Modal::new(id)
            .frame(egui::Frame::popup(ui.style()).inner_margin(self.layout.content_inset()))
            .show(ui.ctx(), |ui| {
                let reserved = 2.0 * f32::from(self.layout.content_inset()) + 24.0;
                ui.set_width(modal_content_width(ui, 650.0, reserved));
                ui.spacing_mut().item_spacing.y = 10.0;
                ui.heading(self.tr(Key::LogReport));
                ui.weak(self.tr(Key::DiagnosticsPrivacy));
                let version = self.message(Key::Version, &[("version", APP_VERSION)]);
                let count = self.message(
                    Key::DiagnosticsCount,
                    &[
                        ("count", &self.snapshot.diagnostics.buffered.to_string()),
                        ("dropped", &self.snapshot.diagnostics.dropped.to_string()),
                    ],
                );
                ui.label(&version);
                ui.label(&count);
                let log = self.log_text(None);
                egui::ScrollArea::both()
                    .max_height((ui.ctx().content_rect().height() / 2.0).min(320.0))
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        if log.is_empty() {
                            ui.weak(self.tr(Key::LogEmpty));
                        } else {
                            ui.add(
                                egui::Label::new(egui::RichText::new(&log).monospace()).extend(),
                            );
                        }
                    });
                ui.separator();
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let width = ((ui.available_width() - ui.spacing().item_spacing.x) / 2.0)
                        .min(CONNECTION_CONTROL_WIDTH);
                    if ui
                        .add(action_button(ui, self.tr(Key::Close), width))
                        .clicked()
                    {
                        ui.close();
                    }
                    if ui
                        .add(action_button(ui, self.tr(Key::LogReportCopy), width))
                        .clicked()
                    {
                        ui.ctx().copy_text(format!(
                            "{}\n{version}\n{count}\n\n{log}",
                            self.tr(Key::AppName)
                        ));
                    }
                });
            })
            .should_close()
    }
}

fn event_labels(kind: ClientEventKind) -> (Key, Key) {
    let application = Key::LogCategoryApplication;
    let subscriptions = Key::LogCategorySubscriptions;
    let profiles = Key::LogCategoryProfiles;
    let connection = Key::LogCategoryConnection;
    match kind {
        ClientEventKind::Started => (application, Key::EventStarted),
        ClientEventKind::SubscriptionUpdated => (subscriptions, Key::EventSubscriptionUpdated),
        ClientEventKind::SubscriptionFallbackUsed => (subscriptions, Key::FallbackUsed),
        ClientEventKind::SubscriptionFailed(error) => (subscriptions, error_key(error)),
        ClientEventKind::SubscriptionSettingsSaved => (subscriptions, Key::LogSettingsSaved),
        ClientEventKind::SubscriptionSettingsFailed(error) => (subscriptions, error_key(error)),
        ClientEventKind::SubscriptionRemoved => (subscriptions, Key::EventSubscriptionRemoved),
        ClientEventKind::ProfilesImported => (profiles, Key::EventProfilesImported),
        ClientEventKind::ProfileUpdated => (profiles, Key::EventProfileUpdated),
        ClientEventKind::ProfileSelected => (profiles, Key::EventProfileSelected),
        ClientEventKind::ProfileRemoved => (profiles, Key::EventProfileRemoved),
        ClientEventKind::ConnectionStarting => (connection, Key::Connecting),
        ClientEventKind::Connected(mode) => (
            connection,
            state_key(ConnectionStatus::Connected, Some(mode)),
        ),
        ClientEventKind::Disconnected => (connection, Key::Disconnected),
        ClientEventKind::SettingsSaved => (Key::LogCategorySettings, Key::LogSettingsSaved),
        ClientEventKind::Failed(error) => (Key::LogCategoryError, error_key(error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vpn_presentation::session::ConnectionMode;

    #[test]
    fn diagnostic_tabs_assign_lifecycle_and_engine_failures_to_core() {
        for event in [
            ClientEventKind::ConnectionStarting,
            ClientEventKind::Connected(ConnectionMode::LocalProxy),
            ClientEventKind::Connected(ConnectionMode::Tun),
            ClientEventKind::Failed(ClientError::EngineReadiness),
        ] {
            assert_eq!(LogTab::source(event), LogTab::Core);
        }
        for event in [
            ClientEventKind::Started,
            ClientEventKind::SettingsSaved,
            ClientEventKind::SubscriptionFailed(ClientError::Network),
        ] {
            assert_eq!(LogTab::source(event), LogTab::Application);
        }
    }
}
