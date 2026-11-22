use super::{
    APP_VERSION, AppUpdateStatus, ConnectionStatus, Key, ViewFrame, divider, group, setting_row,
};
use crate::view::widgets::connection_controls::CONNECTION_CONTROL_WIDTH;
#[cfg(all(debug_assertions, feature = "demo"))]
use crate::view::widgets::controls::SELECTOR_HEIGHT;
use crate::view::widgets::controls::action_button;
use crate::view::widgets::layout::modal_content_width;

const REPOSITORY_URL: &str = "https://github.com/KobaProduction/Blazer";
const CONTENT_INSET: i8 = 10;
const CONTENT_GAP: f32 = 10.0;

impl ViewFrame<'_> {
    pub(in crate::view::widgets) fn about(&self, ui: &mut egui::Ui) {
        let licenses_id = egui::Id::unique("diagnostics-licenses");
        let mut licenses_open = ui
            .ctx()
            .data_mut(|data| data.get_temp::<bool>(licenses_id))
            .unwrap_or_default();
        let rows = [
            (self.tr(Key::AboutVersion), Some(APP_VERSION)),
            (
                self.tr(Key::AboutCoreVersion),
                self.snapshot
                    .component_versions
                    .expected_blazecore
                    .as_deref(),
            ),
            (
                "HWID",
                self.snapshot
                    .subscription_hwid
                    .as_ref()
                    .map(|secret| secret.expose()),
            ),
            (
                self.tr(Key::AboutDevice),
                self.view.host.device_info.as_deref(),
            ),
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
        });
        self.application_update(ui);
        group(ui, None, |ui| {
            setting_row(ui, self.tr(Key::AboutLicenses), |ui| {
                licenses_open |= ui
                    .add(action_button(
                        ui,
                        self.tr(Key::AboutShow),
                        CONNECTION_CONTROL_WIDTH,
                    ))
                    .clicked();
            });
        });

        if licenses_open {
            licenses_open = !self.licenses_dialog(ui, licenses_id);
        }
        ui.ctx()
            .data_mut(|data| data.insert_temp(licenses_id, licenses_open));
    }

    fn licenses_dialog(&self, ui: &egui::Ui, id: egui::Id) -> bool {
        egui::Modal::new(id)
            .frame(egui::Frame::popup(ui.style()).inner_margin(self.layout.content_inset()))
            .show(ui.ctx(), |ui| {
                let reserved = 2.0 * f32::from(self.layout.content_inset()) + 24.0;
                ui.set_width(modal_content_width(ui, 560.0, reserved));
                ui.spacing_mut().item_spacing.y = CONTENT_GAP;
                ui.heading(self.tr(Key::AboutLicenses));
                egui::ScrollArea::vertical()
                    .id_salt("diagnostics-license-content")
                    .max_height((ui.ctx().content_rect().height() * 0.5).min(360.0))
                    .min_scrolled_height(0.0)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = CONTENT_GAP;
                        ui.add(egui::Label::new(self.tr(Key::AboutLicensesDesktop)).wrap());
                        self.license_rows(ui);
                    });
                ui.separator();
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add(action_button(
                            ui,
                            self.tr(Key::Close),
                            CONNECTION_CONTROL_WIDTH,
                        ))
                        .clicked()
                    {
                        ui.close();
                    }
                });
            })
            .should_close()
    }

    fn license_rows(&self, ui: &mut egui::Ui) {
        group(ui, None, |ui| {
            for (index, (component, license, source)) in [
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
            ]
            .into_iter()
            .enumerate()
            {
                if index != 0 {
                    divider(ui);
                }
                setting_row(ui, component, |ui| {
                    ui.hyperlink_to(license, source);
                });
            }
        });
    }

    fn application_update(&self, ui: &mut egui::Ui) {
        group(ui, None, |ui| self.update_content(ui));
    }

    fn update_content(&self, ui: &mut egui::Ui) {
        #[cfg(all(debug_assertions, feature = "demo"))]
        if self.view.preview {
            self.preview_update(ui);
            return;
        }
        if let Some(open) = &self.view.host.open_updates {
            if self.update_check_row(ui, true) {
                open(self.tr(Key::AboutUpdateFailed));
            }
            return;
        }
        let Some(updates) = &self.view.host.app_updates else {
            self.update_check_row(ui, false);
            update_details(ui, |ui| {
                ui.weak(self.tr(Key::AboutUpdateUnavailable));
            });
            return;
        };
        let status = (updates.status)();
        let enabled = !matches!(
            status,
            AppUpdateStatus::Checking | AppUpdateStatus::Downloading
        );
        if self.update_check_row(ui, enabled) {
            (updates.check)();
        }
        if !matches!(status, AppUpdateStatus::Idle) {
            update_details(ui, |ui| self.update_status(ui, status, updates));
        }
    }

    fn update_check_row(&self, ui: &mut egui::Ui, enabled: bool) -> bool {
        let mut clicked = false;
        setting_row(ui, self.tr(Key::SettingsUpdateGroup), |ui| {
            clicked = ui
                .add_enabled(
                    enabled,
                    action_button(ui, self.tr(Key::AboutUpdateCheck), CONNECTION_CONTROL_WIDTH),
                )
                .clicked();
        });
        clicked
    }

    fn update_status(
        &self,
        ui: &mut egui::Ui,
        status: AppUpdateStatus,
        updates: &crate::AppUpdates,
    ) {
        match status {
            AppUpdateStatus::Idle => {}
            AppUpdateStatus::Checking => {
                ui.spinner();
            }
            AppUpdateStatus::Current => {
                ui.label(self.tr(Key::AboutUpdateCurrent));
            }
            AppUpdateStatus::Unpublished => {
                ui.label(self.tr(Key::AboutUpdateUnpublished));
            }
            AppUpdateStatus::Failed => {
                ui.label(self.tr(Key::AboutUpdateFailed));
            }
            AppUpdateStatus::Available {
                version,
                release_url,
            } => {
                ui.label(self.message(Key::AboutUpdateTitle, &[("version", &version)]));
                ui.hyperlink_to(self.tr(Key::AboutUpdateOpenRelease), release_url);
                if let Some(download) = &updates.download
                    && ui
                        .add(action_button(
                            ui,
                            self.tr(Key::AboutUpdateDownload),
                            CONNECTION_CONTROL_WIDTH,
                        ))
                        .clicked()
                {
                    download();
                }
            }
            AppUpdateStatus::Downloading => {
                ui.horizontal_wrapped(|ui| {
                    ui.spinner();
                    ui.add(egui::Label::new(self.tr(Key::AboutUpdateDownloading)).wrap());
                });
            }
            AppUpdateStatus::Ready { version } => {
                self.ready_update(ui, &version, updates);
            }
        }
    }

    fn ready_update(&self, ui: &mut egui::Ui, version: &str, updates: &crate::AppUpdates) {
        ui.label(self.message(Key::AboutUpdateReady, &[("version", version)]));
        let disconnected =
            self.snapshot.connection == ConnectionStatus::Disconnected && !self.snapshot.busy;
        let install = ui.add_enabled(
            disconnected,
            action_button(
                ui,
                self.tr(Key::AboutUpdateInstall),
                CONNECTION_CONTROL_WIDTH,
            ),
        );
        if !disconnected {
            install.on_disabled_hover_text(self.tr(Key::AboutUpdateDisconnect));
        } else if install.clicked() {
            (updates.install)();
        }
    }

    #[cfg(all(debug_assertions, feature = "demo"))]
    fn preview_update(&self, ui: &mut egui::Ui) {
        self.update_check_row(ui, false);
        let id = egui::Id::unique("diagnostics-preview-update-dismissed");
        if ui.ctx().data_mut(|data| data.get_temp::<bool>(id)) == Some(true) {
            return;
        }
        update_details(ui, |ui| {
            ui.label(self.message(Key::AboutUpdateTitle, &[("version", "0.2.0")]));
            ui.label(self.tr(Key::AboutUpdatePreview));
            self.preview_update_buttons(ui, id);
        });
    }

    #[cfg(all(debug_assertions, feature = "demo"))]
    fn preview_update_buttons(&self, ui: &mut egui::Ui, id: egui::Id) {
        let width = ui.available_width().min(CONNECTION_CONTROL_WIDTH);
        let horizontal = width >= 192.0;
        let button_width = if horizontal {
            (width - ui.spacing().item_spacing.x) / 2.0
        } else {
            width
        };
        let buttons = |ui: &mut egui::Ui| {
            if ui
                .add(action_button(
                    ui,
                    self.tr(Key::AboutUpdateSkip),
                    button_width,
                ))
                .clicked()
            {
                ui.ctx().data_mut(|data| data.insert_temp(id, true));
            }
            ui.add_enabled(
                false,
                action_button(ui, self.tr(Key::AboutUpdateDownload), button_width),
            )
            .on_disabled_hover_text(self.tr(Key::Unsupported));
        };
        if horizontal {
            ui.allocate_ui_with_layout(
                egui::vec2(width, SELECTOR_HEIGHT),
                egui::Layout::left_to_right(egui::Align::Center),
                buttons,
            );
        } else {
            buttons(ui);
        }
    }
}

fn update_details(ui: &mut egui::Ui, contents: impl FnOnce(&mut egui::Ui)) {
    divider(ui);
    egui::Frame::new()
        .inner_margin(CONTENT_INSET)
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = CONTENT_GAP;
            contents(ui);
        });
}
