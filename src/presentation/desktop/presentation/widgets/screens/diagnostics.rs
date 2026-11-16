use std::fmt::Write as _;
use std::sync::LazyLock;

use eframe::egui;
use vpn_presentation::localization::Key;
use vpn_presentation::model::{error_key, state_key};
use vpn_presentation::session::{ClientEventKind, ConnectionStatus};

use super::ViewFrame;
use super::settings::{divider, group, setting_row};
use crate::presentation::widgets::controls::more_button;

const MISSING: &str = "—";
const REPOSITORY_URL: &str = "https://github.com/KobaProduction/Blazer";

fn device_info() -> &'static str {
    static DEVICE: LazyLock<String> = LazyLock::new(|| {
        let arch = std::env::consts::ARCH;
        #[cfg(windows)]
        {
            match windows_version() {
                Some(version) => format!("Windows {version} ({arch})"),
                None => format!("Windows ({arch})"),
            }
        }
        #[cfg(not(windows))]
        {
            format!("{} ({arch})", std::env::consts::OS)
        }
    });
    DEVICE.as_str()
}

#[cfg(windows)]
fn windows_version() -> Option<String> {
    let key = windows_registry::LOCAL_MACHINE
        .open(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion")
        .ok()?;
    let major = key.get_u32("CurrentMajorVersionNumber").ok()?;
    let minor = key.get_u32("CurrentMinorVersionNumber").ok()?;
    let build = key
        .get_string("CurrentBuildNumber")
        .ok()?
        .parse::<u32>()
        .ok()?;
    let mut version = format!("{major}.{minor}.{build}");
    if let Ok(revision) = key.get_u32("UBR") {
        let _ = write!(version, ".{revision}");
    }
    Some(version)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
enum LogTab {
    #[default]
    Main,
    Core,
    Tunnel,
    AntiFilter,
    Subscriptions,
    Service,
    Push,
}

impl LogTab {
    const ALL: [Self; 7] = [
        Self::Main,
        Self::Core,
        Self::Tunnel,
        Self::AntiFilter,
        Self::Subscriptions,
        Self::Service,
        Self::Push,
    ];

    fn key(self) -> Key {
        match self {
            Self::Main => Key::LogMain,
            Self::Core => Key::LogCore,
            Self::Tunnel => Key::LogTunnel,
            Self::AntiFilter => Key::LogAntiFilter,
            Self::Subscriptions => Key::LogSubscriptions,
            Self::Service => Key::LogService,
            Self::Push => Key::LogPush,
        }
    }

    fn available(self, preview: bool) -> bool {
        preview || matches!(self, Self::Main | Self::Subscriptions)
    }

    fn includes(self, category: Key) -> bool {
        self == Self::Main
            || (self == Self::Subscriptions && category == Key::LogCategorySubscriptions)
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
                    let log = self.log_text(*selected);
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
        let response = ui
            .add_enabled(tab.available(self.view.preview), button)
            .on_disabled_hover_text(self.tr(Key::LogUnavailable));
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
                        let text = self.log_text(tab);
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

    fn log_text(&self, tab: LogTab) -> String {
        let mut text = String::new();
        for event in &self.snapshot.diagnostic_events {
            let (category, message) = event_labels(event.kind);
            if !tab.includes(category) {
                continue;
            }
            let sequence = event.sequence;
            let category = self.tr(category);
            let message = self.tr(message);
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
        if text.is_empty() && self.view.preview && !tab.available(false) {
            let category = self.tr(tab.key());
            let message = self.tr(Key::LogSampleEvent);
            let _ = writeln!(text, "2026-11-11  12:00:01  #001  [{category}]  {message}");
        }
        text
    }

    fn report_dialog(&self, ui: &egui::Ui, id: egui::Id) -> bool {
        egui::Modal::new(id)
            .show(ui.ctx(), |ui| {
                ui.set_width((ui.ctx().content_rect().width() - 48.0).clamp(120.0, 650.0));
                ui.heading(self.tr(Key::LogReport));
                ui.weak(self.tr(Key::DiagnosticsPrivacy));
                let version = self.message(Key::Version, &[("version", env!("CARGO_PKG_VERSION"))]);
                let count = self.message(
                    Key::DiagnosticsCount,
                    &[
                        ("count", &self.snapshot.diagnostics.buffered.to_string()),
                        ("dropped", &self.snapshot.diagnostics.dropped.to_string()),
                    ],
                );
                ui.label(&version);
                ui.label(&count);
                let log = self.log_text(LogTab::Main);
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
                ui.horizontal(|ui| {
                    if ui.button(self.tr(Key::Close)).clicked() {
                        ui.close();
                    }
                    if ui.button(self.tr(Key::LogReportCopy)).clicked() {
                        ui.ctx().copy_text(format!(
                            "{}\n{version}\n{count}\n\n{log}",
                            self.tr(Key::AppName)
                        ));
                    }
                });
            })
            .should_close()
    }

    pub(super) fn about(&self, ui: &mut egui::Ui) {
        let licenses_id = egui::Id::unique("diagnostics-licenses");
        let mut licenses_open = ui
            .ctx()
            .data_mut(|data| data.get_temp::<bool>(licenses_id))
            .unwrap_or_default();
        let rows = [
            (self.tr(Key::AboutVersion), Some(env!("CARGO_PKG_VERSION"))),
            (
                self.tr(Key::AboutCoreVersion),
                self.snapshot.component_versions.expected_shoes.as_deref(),
            ),
            #[cfg(windows)]
            (
                self.tr(Key::AboutBundledWintun),
                Some(
                    self.snapshot
                        .component_versions
                        .bundled_wintun
                        .as_deref()
                        .unwrap_or_else(|| self.tr(Key::AboutBundledWintunUnavailable)),
                ),
            ),
            (
                self.tr(Key::AboutTunRelay),
                self.snapshot.component_versions.tun2proxy.as_deref(),
            ),
            (
                "HWID",
                self.snapshot
                    .subscription_hwid
                    .as_ref()
                    .map(|secret| secret.expose()),
            ),
            (self.tr(Key::AboutDevice), Some(device_info())),
        ];
        group(ui, None, |ui| {
            for (label, value) in rows {
                setting_row(ui, label, |ui| match value {
                    Some(value) => {
                        ui.add(egui::Label::new(value).truncate())
                            .on_hover_text(value);
                    }
                    None => self.unreported_value(ui),
                });
                divider(ui);
            }
            setting_row(ui, self.tr(Key::AboutRepository), |ui| {
                ui.hyperlink_to("KobaProduction/Blazer", REPOSITORY_URL);
            });
            divider(ui);
            setting_row(ui, self.tr(Key::AboutLicenses), |ui| {
                licenses_open |= ui.button(self.tr(Key::AboutShow)).clicked();
            });
        });

        if self.view.preview {
            ui.add_space(16.0);
            self.preview_update(ui);
        }
        if licenses_open {
            licenses_open = !self.licenses_dialog(ui, licenses_id);
        }
        ui.ctx()
            .data_mut(|data| data.insert_temp(licenses_id, licenses_open));
    }

    fn licenses_dialog(&self, ui: &egui::Ui, id: egui::Id) -> bool {
        egui::Modal::new(id)
            .show(ui.ctx(), |ui| {
                ui.set_width((ui.ctx().content_rect().width() - 48.0).clamp(120.0, 460.0));
                ui.heading(self.tr(Key::AboutLicenses));
                ui.weak(self.tr(Key::AboutLicensesDesktop));
                for (component, license, source) in [
                    ("egui", "MIT OR Apache-2.0", "https://crates.io/crates/egui"),
                    (
                        "eframe",
                        "MIT OR Apache-2.0",
                        "https://crates.io/crates/eframe",
                    ),
                    (
                        "glow",
                        "MIT OR Apache-2.0 OR Zlib",
                        "https://crates.io/crates/glow",
                    ),
                    (
                        "image",
                        "MIT OR Apache-2.0",
                        "https://crates.io/crates/image",
                    ),
                ] {
                    setting_row(ui, component, |ui| {
                        ui.hyperlink_to(license, source);
                    });
                }
                ui.separator();
                if ui.button(self.tr(Key::Close)).clicked() {
                    ui.close();
                }
            })
            .should_close()
    }

    fn preview_update(&self, ui: &mut egui::Ui) {
        let id = egui::Id::unique("diagnostics-preview-update-dismissed");
        if ui.ctx().data_mut(|data| data.get_temp::<bool>(id)) == Some(true) {
            return;
        }
        egui::Frame::new()
            .fill(ui.visuals().faint_bg_color)
            .corner_radius(6)
            .inner_margin(egui::Margin::symmetric(14, 24))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.vertical_centered(|ui| {
                    paint_update_icon(ui);
                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new(
                            self.message(Key::AboutUpdateTitle, &[("version", "0.2.0")]),
                        )
                        .size(18.0)
                        .strong(),
                    );
                    ui.add_space(6.0);
                    ui.label(self.tr(Key::AboutUpdatePreview));
                    ui.add_space(16.0);
                    self.preview_update_buttons(ui, id);
                });
            });
    }

    fn preview_update_buttons(&self, ui: &mut egui::Ui, id: egui::Id) {
        const BUTTON_HEIGHT: f32 = 34.0;
        let width = ui.available_width().min(244.0);
        let horizontal = width >= 192.0;
        let button_width = if horizontal {
            (width - ui.spacing().item_spacing.x) / 2.0
        } else {
            width.min(120.0)
        };
        let buttons = |ui: &mut egui::Ui| {
            let button = |key| {
                egui::Button::new(self.tr(key))
                    .min_size(egui::vec2(button_width, BUTTON_HEIGHT))
                    .wrap_mode(egui::TextWrapMode::Truncate)
            };
            if ui.add(button(Key::AboutUpdateSkip)).clicked() {
                ui.ctx().data_mut(|data| data.insert_temp(id, true));
            }
            ui.add_enabled(false, button(Key::AboutUpdateDownload))
                .on_disabled_hover_text(self.tr(Key::Unsupported));
        };
        if horizontal {
            ui.allocate_ui_with_layout(
                egui::vec2(width, BUTTON_HEIGHT),
                egui::Layout::left_to_right(egui::Align::Center),
                buttons,
            );
        } else {
            buttons(ui);
        }
    }
}

fn paint_update_icon(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(56.0, 56.0), egui::Sense::hover());
    let center = rect.center();
    let painter = ui.painter();
    let stroke = egui::Stroke::new(2.0, ui.visuals().hyperlink_color);
    painter.circle_filled(center, 27.0, ui.visuals().selection.bg_fill);
    painter.circle_stroke(center, 27.0, stroke);
    painter.line_segment(
        [
            center + egui::vec2(0.0, 11.0),
            center + egui::vec2(0.0, -11.0),
        ],
        stroke,
    );
    painter.add(egui::Shape::line(
        [(-8.0, -3.0), (0.0, -11.0), (8.0, -3.0)]
            .map(|(x, y)| center + egui::vec2(x, y))
            .to_vec(),
        stroke,
    ));
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
