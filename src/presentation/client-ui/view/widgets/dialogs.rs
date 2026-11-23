use std::borrow::Cow;

use egui::class::HasClasses;

use super::ViewFrame;
use super::controls::{feedback, paint_spinner, roomy_input};
use super::flags::server_name;
use super::layout::{ViewportLayout, modal_content_width};
use super::screens::{toggle, toggle_width};
use crate::view::view_model::{ConfigurationDraft, ConfigurationKind, ProfileEditor};
use vpn_presentation::import::{self, ImportKind};
use vpn_presentation::localization::{Key, Locale};
use vpn_presentation::model::{ErrorNotice, Intent, Removal, profile_details_hidden};
use vpn_presentation::session::{ProfileSummary, Secret};

const DIALOG_WIDTH: f32 = 728.0;
const DIALOG_PADDING: i8 = 28;
const FIELD_HEIGHT: f32 = 32.0;
const FOOTER_HEIGHT: f32 = 36.0;

impl ViewFrame<'_> {
    pub(super) fn tun_start_error_dialog(&mut self, ui: &egui::Ui) {
        let Some(notice) = self.view.tun_start_notice.current() else {
            return;
        };
        let modal = egui::Modal::new(egui::Id::unique("tun-start-error"))
            .frame(dialog_frame(ui, 20))
            .show(ui.ctx(), |ui| {
                ui.set_width(modal_content_width(ui, 420.0, 64.0));
                ui.heading(self.tr(Key::CatalogBlazeCoreTun));
                ui.add_space(8.0);
                ui.add(egui::Label::new(notice.text(self.view.locale)).wrap());
                ui.add_space(12.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add(super::controls::action_button(
                        ui,
                        self.tr(Key::Close),
                        104.0,
                    ))
                    .clicked()
                })
                .inner
            });
        if modal.inner || modal.should_close() {
            self.view.tun_start_notice.dismiss();
            ui.ctx().request_repaint();
        }
    }

    pub(super) fn subscription_error_dialog(&mut self, ui: &egui::Ui) {
        let Some(error) = self.view.subscription_errors.current() else {
            return;
        };
        let notice = ErrorNotice::Client(error);
        let modal = egui::Modal::new(egui::Id::unique("subscription-refresh-error"))
            .frame(dialog_frame(ui, 20))
            .show(ui.ctx(), |ui| {
                ui.set_width(modal_content_width(ui, 400.0, 64.0));
                ui.heading(self.tr(Key::SubscriptionRefreshErrorTitle));
                ui.add(
                    egui::Label::new(notice.text_with_code(self.view.locale, notice.key())).wrap(),
                );
                ui.add_space(8.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.button(self.tr(Key::Close)).clicked()
                })
                .inner
            });
        if modal.inner || modal.should_close() {
            self.view.subscription_errors.dismiss();
            ui.ctx().request_repaint();
        }
    }

    pub(super) fn add_menu(&mut self, ui: &mut egui::Ui) {
        ui.set_min_width(modal_content_width(ui, 196.0, 24.0));
        ui.set_max_width(modal_content_width(ui, 320.0, 24.0));
        ui.add_enabled_ui(!self.snapshot.busy, |ui| {
            for kind in [
                ConfigurationKind::Subscription,
                ConfigurationKind::Configuration,
                ConfigurationKind::Clipboard,
                ConfigurationKind::Qr,
            ] {
                if ui.button(self.tr(kind.key())).clicked() {
                    self.view.open_configuration(kind);
                    if kind == ConfigurationKind::Clipboard
                        && let Some(draft) = &mut self.view.configuration
                    {
                        draft.source.paste(ui.ctx());
                    }
                    ui.close();
                }
            }
        });
    }

    pub(super) fn configuration_dialog(&mut self, ui: &mut egui::Ui) {
        let Some(mut draft) = self.view.configuration.take() else {
            return;
        };
        let automatic = self.read_import_source(ui, &mut draft);
        let title = draft.kind.key();
        let pending = draft.pending.is_some();
        let viewport = ui.ctx().content_rect();
        let padding = if self.layout == ViewportLayout::Mobile || viewport.height() < 420.0 {
            14
        } else {
            DIALOG_PADDING
        };
        let modal = egui::Modal::new(egui::Id::unique("configuration-dialog"))
            .frame(dialog_frame(ui, padding))
            .show(ui.ctx(), |ui| {
                self.configuration_contents(ui, &mut draft, title, padding)
            });
        if !pending && (modal.inner == FormAction::Close || modal.should_close()) {
            if draft.feedback.is_some() {
                self.view.dismissed_error = draft.feedback;
            }
            return;
        }
        if (automatic || modal.inner == FormAction::Submit)
            && let Some(intent) = configuration_intent(&draft, self.view.locale)
        {
            self.emit(intent);
        }
        self.view.configuration = Some(draft);
    }

    fn configuration_error(&self, draft: &ConfigurationDraft) -> Option<Cow<'static, str>> {
        if let Some(key) = draft.source.error {
            return Some(Cow::Borrowed(self.tr(key)));
        }
        let (_, notice) = draft.feedback?;
        let key = notice.key();
        let key = draft
            .kind
            .import_kind()
            .map_or(key, |kind| kind.error_message(key, &draft.input));
        Some(notice.text_with_code(self.view.locale, key))
    }

    fn read_import_source(&self, ui: &mut egui::Ui, draft: &mut ConfigurationDraft) -> bool {
        if draft.kind == ConfigurationKind::Configuration {
            if draft.source.busy()
                && let Some(input) = draft.source.poll(ui.ctx(), false)
            {
                draft.input = input;
                draft.feedback = None;
            }
            return false;
        }
        if draft.kind.import_kind().is_some() {
            return false;
        }
        let image_only = draft.kind == ConfigurationKind::Qr;
        let Some(input) = draft.source.poll(ui.ctx(), image_only) else {
            return false;
        };
        let confirm_link = import::import_link(&input).is_some();
        set_imported_input(draft, input, self.view.locale);
        !self.snapshot.busy && !confirm_link
    }

    fn configuration_contents(
        &self,
        ui: &mut egui::Ui,
        draft: &mut ConfigurationDraft,
        title: Key,
        padding: i8,
    ) -> FormAction {
        let viewport = ui.ctx().content_rect();
        let width = modal_content_width(ui, DIALOG_WIDTH, 2.0 * f32::from(padding) + 24.0);
        ui.set_width(width);
        ui.spacing_mut().scroll = egui::style::ScrollStyle::solid();
        let pending = draft.pending.is_some();
        let close = ui
            .horizontal(|ui| {
                let close_width = 35.0 + ui.spacing().item_spacing.x;
                ui.allocate_ui_with_layout(
                    egui::vec2((ui.available_width() - close_width).max(0.0), FIELD_HEIGHT),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(self.tr(title)).size(22.0).strong(),
                            )
                            .truncate(),
                        );
                    },
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_enabled(
                        !pending,
                        egui::Button::new(egui::RichText::new("×").size(23.0)).frame(false),
                    )
                    .on_hover_text(self.tr(Key::Close))
                    .clicked()
                })
                .inner
            })
            .inner;
        if close {
            return FormAction::Close;
        }
        let gap = if padding == DIALOG_PADDING { 16.0 } else { 8.0 };
        let chrome_height = FIELD_HEIGHT + FOOTER_HEIGHT + 2.0 * (f32::from(padding) + gap) + 26.0;
        ui.add_space(gap);
        egui::ScrollArea::vertical()
            .max_height((viewport.height() - chrome_height).max(38.0))
            .auto_shrink([false, true])
            .show(ui, |ui| {
                ui.add_enabled_ui(!pending, |ui| self.configuration_fields(ui, draft));
                if let Some(message) = self.configuration_error(draft) {
                    ui.add_space(8.0);
                    feedback(ui, &message, None);
                }
            });
        if draft.kind.import_kind().is_none() {
            return FormAction::None;
        }
        ui.add_space(gap);
        self.configuration_footer(ui, draft)
    }

    fn configuration_fields(&self, ui: &mut egui::Ui, draft: &mut ConfigurationDraft) {
        ui.spacing_mut().interact_size.y = FIELD_HEIGHT;
        match draft.kind {
            ConfigurationKind::Subscription => self.subscription_fields(ui, draft),
            ConfigurationKind::Configuration => {
                self.form_field(ui, Key::ConfigurationName, &mut draft.title, false);
                ui.add_space(8.0);
                ui.label(self.tr(Key::ConfigurationText));
                ui.add(
                    egui::TextEdit::multiline(&mut draft.input)
                        .desired_width(f32::INFINITY)
                        .desired_rows(8)
                        .char_limit(import::MAX_TEXT_BYTES),
                );
            }
            ConfigurationKind::Clipboard => self.clipboard_fields(ui, draft),
            ConfigurationKind::Qr => self.qr_fields(ui, draft),
        }
    }

    fn configuration_file_picker(&self, ui: &mut egui::Ui, draft: &mut ConfigurationDraft) {
        let Some(pick) = &self.view.host.pick_configuration else {
            return;
        };
        let reading = draft.source.busy();
        let button = egui::Button::new(self.tr(Key::ConfigurationOpenFile))
            .frame(false)
            .min_size(egui::vec2(110.0, 34.0));
        let response = ui.add_enabled(!reading && draft.pending.is_none(), button);
        if reading {
            response.on_disabled_hover_text(self.tr(Key::ImportReading));
            ui.spinner();
        } else if response.clicked() {
            draft.source.pick_file(
                ui.ctx(),
                self.tr(Key::ConfigurationOpenFile),
                Key::ImportUnreadableConfiguration,
                pick.clone(),
            );
        }
    }

    fn clipboard_fields(&self, ui: &mut egui::Ui, draft: &mut ConfigurationDraft) {
        if draft.source.busy() {
            ui.horizontal_wrapped(|ui| {
                ui.spinner();
                ui.label(self.tr(Key::ImportReading));
            });
        } else if ui.button(self.tr(Key::Retry)).clicked() {
            draft.source.paste(ui.ctx());
        }
    }

    fn qr_fields(&self, ui: &mut egui::Ui, draft: &mut ConfigurationDraft) {
        if draft.source.busy() {
            ui.horizontal_wrapped(|ui| {
                ui.spinner();
                ui.label(self.tr(Key::ImportReading));
            });
            return;
        }
        if let Some(path) = ui.input(|input| {
            input
                .raw
                .dropped_files
                .first()
                .map(|file| file.path().to_owned())
        }) {
            draft.source.open_file(ui.ctx(), path);
        }
        ui.horizontal_wrapped(|ui| {
            if let Some(scan) = &self.view.host.scan_qr_camera
                && ui.button(self.tr(Key::QrCamera)).clicked()
            {
                let opened = scan(
                    self.tr(Key::QrCamera),
                    self.tr(Key::QrCameraHelp),
                    self.tr(Key::Cancel),
                    self.tr(Key::QrCameraUnavailable),
                );
                draft.source.error = (!opened).then_some(Key::QrCameraUnavailable);
            }
            if let Some(pick) = &self.view.host.pick_qr_image
                && ui.button(self.tr(Key::QrOpenFile)).clicked()
            {
                draft.source.pick_file(
                    ui.ctx(),
                    self.tr(Key::QrOpenFile),
                    Key::QrUnreadableImage,
                    pick.clone(),
                );
            }
            if let Some(capture) = &self.view.host.capture_qr_image
                && ui.button(self.tr(Key::QrScreenshot)).clicked()
            {
                draft.source.screenshot_help = true;
                draft.source.capture_screen(ui.ctx(), capture.clone());
            }
        });
        if draft.source.screenshot_help {
            ui.add_space(8.0);
            ui.weak(self.tr(Key::QrScreenshotHelp));
        }
        if (draft.source.screenshot_help || self.view.host.capture_qr_image.is_none())
            && ui.button(self.tr(Key::PasteQrImage)).clicked()
        {
            draft.source.paste(ui.ctx());
        }
        #[cfg(all(debug_assertions, feature = "demo"))]
        if self.view.preview && ui.button(self.tr(Key::QrDemoChoose)).clicked() {
            set_imported_input(
                draft,
                "socks5://203.0.113.4:1080".to_owned(),
                self.view.locale,
            );
            draft.title = self.tr(Key::QrDemoServer).to_owned();
        }
    }

    fn subscription_fields(&self, ui: &mut egui::Ui, draft: &mut ConfigurationDraft) {
        self.form_field(ui, Key::SubscriptionName, &mut draft.title, false);
        ui.weak(self.tr(Key::SubscriptionAutomaticName));
        ui.add_space(8.0);
        self.form_field(ui, Key::SubscriptionUrl, &mut draft.input, true);
        ui.add_space(10.0);
        let rows = [
            (Key::HideServerSettings, Key::HideServerSettingsHelp, false),
            (
                Key::EncryptedSubscription,
                Key::EncryptedSubscriptionHelp,
                false,
            ),
            (
                Key::AllowUnsafeConnection,
                Key::AllowUnsafeConnectionHelp,
                false,
            ),
        ];
        for (label, help, value) in rows {
            let mut indicator = value;
            ui.separator();
            ui.horizontal(|ui| {
                let control = toggle_width(ui, self.view.locale, indicator);
                let text_width =
                    (ui.available_width() - control - ui.spacing().item_spacing.x).max(1.0);
                ui.allocate_ui_with_layout(
                    egui::vec2(text_width, 43.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.add(
                            egui::Label::new(egui::RichText::new(self.tr(label)).strong()).wrap(),
                        );
                        ui.add(egui::Label::new(egui::RichText::new(self.tr(help)).weak()).wrap());
                    },
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_enabled_ui(false, |ui| {
                        toggle(ui, self.view.locale, label, &mut indicator)
                            .on_disabled_hover_text(self.tr(Key::SettingUnavailable))
                    });
                });
            });
        }
    }

    fn form_field(&self, ui: &mut egui::Ui, key: Key, value: &mut String, password: bool) {
        ui.weak(self.tr(key));
        ui.add(
            roomy_input(value, ui.available_width())
                .id(form_field_id(key))
                .password(password),
        );
    }

    fn configuration_footer(
        &self,
        ui: &mut egui::Ui,
        draft: &mut ConfigurationDraft,
    ) -> FormAction {
        let key = Key::Add;
        let complete = !import::payload(&draft.input).is_empty();
        let can_submit =
            !self.snapshot.busy && draft.pending.is_none() && !draft.source.busy() && complete;
        ui.allocate_ui_with_layout(
            egui::vec2(ui.available_width(), FOOTER_HEIGHT),
            egui::Layout::right_to_left(egui::Align::Center),
            |ui| {
                let mut button = egui::Button::new(self.tr(key)).min_size(egui::vec2(110.0, 34.0));
                button.set_class(crate::view::theme::DISABLED_BUTTON, !can_submit);
                let response = ui.add_enabled(can_submit, button);
                let response = if draft.pending.is_some() {
                    let center = response.rect.left_center() + egui::vec2(15.0, 0.0);
                    paint_spinner(ui, center, 14.0);
                    response.on_disabled_hover_text(self.tr(Key::Working))
                } else {
                    response
                };
                if draft.kind == ConfigurationKind::Configuration {
                    self.configuration_file_picker(ui, draft);
                }
                if response.clicked() {
                    FormAction::Submit
                } else {
                    FormAction::None
                }
            },
        )
        .inner
    }

    pub(super) fn profile_editor_dialog(&mut self, ui: &mut egui::Ui) {
        let Some(mut draft) = self.view.profile_editor.take() else {
            return;
        };
        let Some(profile) = self
            .snapshot
            .profiles
            .iter()
            .find(|row| row.profile_uuid == draft.uuid)
        else {
            return;
        };
        let pending = draft.pending.is_some();
        let modal = egui::Modal::new(egui::Id::unique("profile-editor"))
            .frame(dialog_frame(ui, 16))
            .show(ui.ctx(), |ui| {
                self.profile_editor_contents(ui, profile, &mut draft)
            });
        if !pending && modal.should_close() {
            self.view.dismissed_error = draft.feedback;
            return;
        }
        if modal.inner {
            self.emit(Intent::UpdateProfile {
                profile_uuid: draft.uuid.clone(),
                text: Secret::new(import::payload(&draft.text)),
            });
        }
        self.view.profile_editor = Some(draft);
    }

    fn profile_editor_contents(
        &mut self,
        ui: &mut egui::Ui,
        profile: &ProfileSummary,
        draft: &mut ProfileEditor,
    ) -> bool {
        ui.set_width(modal_content_width(ui, DIALOG_WIDTH, 56.0));
        ui.horizontal(|ui| {
            let width = (ui.available_width() - 32.0).max(0.0);
            ui.add_sized(
                [width, FIELD_HEIGHT],
                egui::Label::new(egui::RichText::new(self.tr(Key::EditConfiguration)).size(22.0))
                    .truncate(),
            );
            if ui
                .add_enabled(draft.pending.is_none(), egui::Button::new("×"))
                .on_hover_text(self.tr(Key::Close))
                .clicked()
            {
                ui.close();
            }
        });
        ui.add(egui::Label::new(server_name(&profile.name)).truncate());
        ui.add_space(6.0);
        let hidden = profile_details_hidden(self.snapshot, profile);
        if hidden {
            draft.text.clear();
            ui.label(self.tr(Key::ProfileHidden));
        } else {
            self.profile_source_field(ui, profile, draft);
        }
        ui.add_space(8.0);
        self.profile_editor_actions(ui, profile, draft, hidden)
    }

    fn profile_source_field(
        &self,
        ui: &mut egui::Ui,
        profile: &ProfileSummary,
        draft: &mut ProfileEditor,
    ) {
        let label = if profile.protocol == "AmneziaWG" && profile.source.is_some() {
            Key::AmneziaProfileParameters
        } else if profile.source.is_some() {
            Key::ProfileSource
        } else {
            Key::ProfileSourceMissing
        };
        ui.weak(self.tr(label));
        let height = (ui.ctx().content_rect().height() - 240.0).clamp(80.0, 410.0);
        egui::ScrollArea::both().max_height(height).show(ui, |ui| {
            let edit = egui::TextEdit::multiline(&mut draft.text)
                .id_salt(("profile-source", &draft.uuid))
                .code_editor()
                .desired_rows(12)
                .desired_width(ui.available_width())
                .char_limit(import::MAX_TEXT_BYTES);
            if ui.add_enabled(draft.pending.is_none(), edit).changed() {
                draft.feedback = None;
            }
        });
        if let Some((_, error)) = draft.feedback {
            feedback(
                ui,
                &error.text_with_code(self.view.locale, error.key()),
                None,
            );
        }
    }

    fn profile_editor_actions(
        &mut self,
        ui: &mut egui::Ui,
        profile: &ProfileSummary,
        draft: &ProfileEditor,
        hidden: bool,
    ) -> bool {
        let active = self.snapshot.active_profile_uuid.as_ref() == Some(&draft.uuid);
        let enabled = !active && !self.snapshot.busy && draft.pending.is_none();
        ui.horizontal_wrapped(|ui| {
            if draft.pending.is_some() {
                ui.spinner();
            } else {
                self.profile_ping_button(ui, Some(profile), false);
            }
            if ui
                .add_enabled(enabled, egui::Button::new(self.tr(Key::Remove)))
                .clicked()
            {
                self.view.removal = Some(Removal::Profile(draft.uuid.clone()));
                ui.close();
            }
            if hidden {
                return false;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let text = import::payload(&draft.text);
                let changed = !text.is_empty()
                    && profile
                        .source
                        .as_ref()
                        .is_none_or(|source| source.expose() != text);
                let button =
                    ui.add_enabled(enabled && changed, egui::Button::new(self.tr(Key::Save)));
                if active {
                    button
                        .on_disabled_hover_text(self.tr(Key::DisconnectFirst))
                        .clicked()
                } else {
                    button.clicked()
                }
            })
            .inner
        })
        .inner
    }

    pub(super) fn remove_confirmation(&mut self, ui: &mut egui::Ui) {
        let Some(removal) = self.view.removal.clone() else {
            return;
        };
        let key = match removal {
            Removal::Subscription(_) => Key::RemoveSubscriptionConfirm,
            Removal::Profile(_) => Key::ProfileRemoved,
        };
        let dialog = egui::Modal::new(egui::Id::unique("remove-confirmation"))
            .frame(dialog_frame(ui, 20))
            .show(ui.ctx(), |ui| self.remove_confirmation_contents(ui, key));
        if dialog.inner == FormAction::Submit {
            self.emit(Intent::ConfirmRemoval(removal));
        }
        if dialog.inner == FormAction::Close || dialog.should_close() {
            self.view.removal = None;
        }
    }

    fn remove_confirmation_contents(&self, ui: &mut egui::Ui, question: Key) -> FormAction {
        ui.set_width(modal_content_width(ui, 360.0, 40.0));
        ui.vertical_centered(|ui| {
            ui.add(egui::Label::new(self.tr(question)).wrap());
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                let size = egui::vec2(104.0, 34.0);
                if ui
                    .add(
                        egui::Button::new(self.tr(Key::Cancel))
                            .frame_when_inactive(false)
                            .min_size(size),
                    )
                    .clicked()
                {
                    return FormAction::Close;
                }
                if ui
                    .add_enabled(
                        !self.snapshot.busy,
                        egui::Button::new(self.tr(Key::ConfirmRemove)).min_size(size),
                    )
                    .clicked()
                {
                    return FormAction::Submit;
                }
                FormAction::None
            })
            .inner
        })
        .inner
    }
}

#[derive(PartialEq, Eq)]
enum FormAction {
    None,
    Close,
    Submit,
}

fn set_imported_input(draft: &mut ConfigurationDraft, input: String, locale: Locale) {
    let kind = import::detect(&input);
    draft.kind = match kind {
        ImportKind::Subscription => ConfigurationKind::Subscription,
        ImportKind::Configuration => ConfigurationKind::Configuration,
    };
    if kind == ImportKind::Configuration && draft.title.is_empty() {
        draft.title = kind.default_title(locale).to_owned();
    }
    draft.input = input;
    draft.source = Default::default();
}

fn dialog_frame(ui: &egui::Ui, padding: i8) -> egui::Frame {
    let visuals = ui.visuals();
    egui::Frame::popup(ui.style())
        .fill(
            visuals
                .panel_fill
                .lerp_to_gamma(visuals.faint_bg_color, 0.46),
        )
        .inner_margin(padding)
}

fn form_field_id(key: Key) -> egui::Id {
    egui::Id::unique(("configuration-dialog-field", key.id()))
}

fn configuration_intent(draft: &ConfigurationDraft, locale: Locale) -> Option<Intent> {
    Some(
        draft
            .kind
            .import_kind()?
            .intent(locale, &draft.title, &draft.input),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::view_model::ViewState;
    use vpn_presentation::session::ClientSnapshot;

    #[test]
    fn clipboard_action_routes_a_subscription_and_a_base64_configuration() {
        let subscription = paste_import("https://provider.example/subscription");
        assert!(matches!(subscription, Intent::SaveSubscription {
            subscription_uuid: None, title, url
        } if title.is_empty() && url == "https://provider.example/subscription"));

        let encoded = "c29ja3M1Oi8vZXhhbXBsZS50ZXN0OjEwODA=";
        assert!(
            matches!(paste_import(encoded), Intent::ImportProfiles { text, .. } if text == encoded)
        );
    }

    #[test]
    fn subscription_form_keeps_a_manually_entered_title() {
        let draft = ConfigurationDraft {
            kind: ConfigurationKind::Subscription,
            title: "My subscription".to_owned(),
            input: "https://provider.example/subscription".to_owned(),
            ..Default::default()
        };
        assert!(matches!(
            configuration_intent(&draft, Locale::English),
            Some(Intent::SaveSubscription { subscription_uuid: None, title, .. })
                if title == "My subscription"
        ));
    }

    #[test]
    fn configuration_footer_keeps_file_open_beside_add_at_desktop_and_narrow_widths() {
        for width in [360.0, 1100.0] {
            let context = egui::Context::default();
            context.global_style_mut(|style| style.animation_time = 0.0);
            let mut state = ViewState::default();
            state.locale = Locale::English;
            state.host.pick_configuration = Some(std::sync::Arc::new(|_| Ok(None)));
            state.open_configuration(ConfigurationKind::Configuration);
            let snapshot = ClientSnapshot {
                busy: false,
                ..ClientSnapshot::default()
            };
            let mut view = ViewFrame::new(&mut state, &snapshot, false, None);
            let mut shapes = Vec::new();
            for _ in 0..6 {
                let mut output = context.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(width, 640.0),
                        )),
                        ..Default::default()
                    },
                    |ui| view.configuration_dialog(ui),
                );
                output.textures_delta.clear();
                shapes = output.shapes;
            }
            let labels = shapes.iter().map(|item| &item.shape);
            let open = labels
                .clone()
                .find_map(|shape| label_rect(shape, "Open file"))
                .unwrap();
            let add = labels
                .clone()
                .find_map(|shape| label_rect(shape, "Add"))
                .unwrap();
            let field = labels
                .clone()
                .find_map(|shape| label_rect(shape, "Configuration text or share links"))
                .unwrap();
            assert!((open.center().y - add.center().y).abs() < 1.0);
            assert!(open.right() < add.left());
            assert!(open.top() > field.bottom());
            assert!(open.left() >= 0.0 && add.right() <= width);
        }
    }

    #[test]
    fn removal_dialog_blocks_background_clicks_and_stays_centered() {
        let context = egui::Context::default();
        context.global_style_mut(|style| style.animation_time = 0.0);
        let mut state = ViewState::default();
        state.locale = Locale::English;
        state.removal = Some(Removal::Profile("fixture".into()));
        let snapshot = ClientSnapshot {
            busy: false,
            ..ClientSnapshot::default()
        };
        let mut view = ViewFrame::new(&mut state, &snapshot, false, None);
        let mut clicked_background = false;
        let background = egui::pos2(20.0, 20.0);
        let mut draw = |events| {
            let mut output = context.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(720.0, 600.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| {
                    let rect = egui::Rect::from_center_size(background, egui::vec2(32.0, 28.0));
                    clicked_background |= ui.put(rect, egui::Button::new("X")).clicked();
                    view.remove_confirmation(ui);
                },
            );
            output.textures_delta.clear();
            output.shapes
        };
        for _ in 0..6 {
            draw(Vec::new());
        }
        let shapes = draw(Vec::new());
        assert!(
            shapes
                .iter()
                .any(|item| label_rect(&item.shape, "Yes, remove").is_some())
        );
        let question = shapes
            .iter()
            .find_map(|item| label_rect(&item.shape, "Remove this configuration?"))
            .unwrap();
        assert!((question.center().x - 360.0).abs() < 10.0);
        for pressed in [true, false] {
            draw(vec![
                egui::Event::PointerMoved(background),
                egui::Event::PointerButton {
                    pos: background,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ]);
        }
        assert!(!clicked_background);
        assert!(view.intent.is_none());
    }

    fn label_rect(shape: &egui::Shape, label: &str) -> Option<egui::Rect> {
        match shape {
            egui::Shape::Text(text) if text.galley.job.text == label => {
                Some(text.galley.rect.translate(text.pos.to_vec2()))
            }
            egui::Shape::Vec(shapes) => shapes.iter().find_map(|shape| label_rect(shape, label)),
            _ => None,
        }
    }

    fn paste_import(payload: &str) -> Intent {
        paste_into_import(payload)
            .0
            .expect("clipboard import intent")
    }

    #[test]
    fn blazer_clipboard_link_waits_for_confirmation() {
        let payload = "blazer://add/https://provider.example/subscription";
        let (intent, state) = paste_into_import(payload);
        assert!(intent.is_none());
        let draft = state.configuration.expect("import form populated");
        assert!(matches!(
            configuration_intent(&draft, Locale::English),
            Some(Intent::SaveSubscription { title, url, .. })
                if title.is_empty() && url == "https://provider.example/subscription"
        ));
    }

    fn paste_into_import(payload: &str) -> (Option<Intent>, ViewState) {
        let context = egui::Context::default();
        let mut state = ViewState::default();
        state.open_configuration(ConfigurationKind::Clipboard);
        let snapshot = ClientSnapshot {
            busy: false,
            ..ClientSnapshot::default()
        };
        let mut view = ViewFrame::new(&mut state, &snapshot, false, None);
        let mut output = context.run_ui(
            egui::RawInput {
                events: vec![egui::Event::Paste(payload.to_owned())],
                ..Default::default()
            },
            |ui| view.configuration_dialog(ui),
        );
        output.textures_delta.clear();
        (view.intent, state)
    }

    #[test]
    fn subscription_url_receives_paste_with_the_same_field_layout_as_name() {
        let context = egui::Context::default();
        context.global_style_mut(|style| style.animation_time = 0.0);
        let mut state = ViewState::default();
        state.open_configuration(ConfigurationKind::Subscription);
        let snapshot = ClientSnapshot {
            busy: false,
            ..ClientSnapshot::default()
        };
        let mut view = ViewFrame::new(&mut state, &snapshot, false, None);
        let mut render = |events| {
            let mut output = context.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1014.0, 880.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| view.configuration_dialog(ui),
            );
            output.textures_delta.clear();
            output.shapes
        };
        for _ in 0..6 {
            render(Vec::new());
        }
        let url_id = form_field_id(Key::SubscriptionUrl);
        let name_id = form_field_id(Key::SubscriptionName);
        let url = context.read_response(url_id).unwrap().rect;
        let name = context.read_response(name_id).unwrap().rect;
        assert_eq!(url.size(), name.size());
        assert_eq!(url.left(), name.left());
        let pos = url.center();
        render(vec![egui::Event::PointerMoved(pos)]);
        for pressed in [true, false] {
            render(vec![egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            }]);
        }
        let response = context.read_response(url_id).unwrap();
        assert!(
            response.has_focus(),
            "URL field at {pos:?} did not receive focus: {response:?}"
        );
        let sample = "https://provider.example/subscription";
        let shapes = render(vec![egui::Event::Paste(sample.to_owned())]);
        let draft = view.view.configuration.as_ref().unwrap();
        assert_eq!(draft.input, sample);
        assert!(draft.title.is_empty());
        assert!(
            shapes
                .iter()
                .all(|shape| hides_sample(&shape.shape, sample))
        );
    }

    fn hides_sample(shape: &egui::Shape, sample: &str) -> bool {
        match shape {
            egui::Shape::Text(text) => !text.galley.job.text.contains(sample),
            egui::Shape::Vec(shapes) => shapes.iter().all(|shape| hides_sample(shape, sample)),
            _ => true,
        }
    }
}
