use std::borrow::Cow;

use egui::class::HasClasses;

use super::ViewFrame;
use super::controls::{feedback, paint_spinner, roomy_input};
use super::flags::server_name;
use super::layout::{ViewportLayout, modal_content_width};
use super::screens::toggle;
use crate::view::view_model::{ConfigurationDraft, ConfigurationKind};
use vpn_presentation::import::{self, ImportKind};
use vpn_presentation::localization::{Key, Locale};
use vpn_presentation::model::{ErrorNotice, Intent, Removal, profile_details_hidden};

const DIALOG_WIDTH: f32 = 728.0;
const DIALOG_PADDING: i8 = 28;
const FIELD_HEIGHT: f32 = 32.0;
const FOOTER_HEIGHT: f32 = 36.0;

impl ViewFrame<'_> {
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
            if let Some(pick) = &self.view.host.pick_qr_image
                && ui.button(self.tr(Key::QrOpenFile)).clicked()
            {
                draft
                    .source
                    .pick_file(ui.ctx(), self.tr(Key::QrOpenFile), pick.clone());
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
                let text_width = (ui.available_width() - 66.0).max(1.0);
                ui.allocate_ui_with_layout(
                    egui::vec2(text_width, 43.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.strong(self.tr(label));
                        ui.weak(self.tr(help));
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

    fn configuration_footer(&self, ui: &mut egui::Ui, draft: &ConfigurationDraft) -> FormAction {
        let key = Key::Add;
        let complete = !import::payload(&draft.input).is_empty();
        let can_submit = !self.snapshot.busy && draft.pending.is_none() && complete;
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
        let hidden = profile_details_hidden(self.snapshot, profile);
        let modal = egui::Modal::new(egui::Id::unique("server-json-editor"))
            .frame(dialog_frame(ui, 16))
            .show(ui.ctx(), |ui| {
                let viewport = ui.ctx().content_rect();
                ui.set_width(modal_content_width(ui, DIALOG_WIDTH, 56.0));
                ui.horizontal(|ui| {
                    let title = self.tr(Key::EditConfiguration);
                    let title_width = (ui.available_width() - 32.0).max(1.0);
                    ui.add_sized(
                        [title_width, FIELD_HEIGHT],
                        egui::Label::new(egui::RichText::new(title).size(22.0).strong()).truncate(),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("×").on_hover_text(self.tr(Key::Close)).clicked() {
                            ui.close();
                        }
                    });
                });
                ui.add(egui::Label::new(server_name(&profile.name)).truncate());
                ui.add_space(6.0);
                if hidden {
                    ui.label(self.tr(Key::ProfileHidden));
                } else {
                    ui.weak(self.tr(Key::ProfileJsonDraft));
                    ui.add_space(6.0);
                    egui::ScrollArea::both()
                        .max_height((viewport.height() - 240.0).clamp(80.0, 410.0))
                        .show(ui, |ui| {
                            let edit = egui::TextEdit::multiline(&mut draft.json)
                                .id_salt("core-json")
                                .code_editor()
                                .desired_rows(13)
                                .desired_width(ui.available_width());
                            if ui.add(edit).changed() {
                                draft.invalid = false;
                            }
                        });
                    if draft.invalid {
                        ui.colored_label(
                            ui.visuals().error_fg_color,
                            self.tr(Key::ProfileJsonInvalid),
                        );
                    }
                }
                ui.add_space(8.0);
                ui.horizontal_wrapped(|ui| {
                    self.profile_ping_button(ui, Some(profile), false);
                    let can_remove = !self.snapshot.busy
                        && self.snapshot.active_profile_uuid.as_ref() != Some(&draft.uuid);
                    if ui
                        .add_enabled(can_remove, egui::Button::new(self.tr(Key::Remove)))
                        .clicked()
                    {
                        self.view.removal = Some(Removal::Profile(draft.uuid.clone()));
                        ui.close();
                    }
                    if !hidden {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.button(self.tr(Key::Save)).clicked()
                        })
                        .inner
                    } else {
                        false
                    }
                })
                .inner
            });
        if modal.should_close() {
            return;
        }
        if modal.inner {
            match serde_json::from_str::<serde_json::Value>(&draft.json) {
                Ok(value) if value.is_object() || value.is_array() => {
                    self.view.profile_drafts.insert(draft.uuid, draft.json);
                    return;
                }
                _ => draft.invalid = true,
            }
        }
        self.view.profile_editor = Some(draft);
    }

    pub(super) fn remove_confirmation(&mut self, ui: &mut egui::Ui) {
        let Some(removal) = self.view.removal.clone() else {
            return;
        };
        if ui.ctx().input(|input| input.key_pressed(egui::Key::Escape)) {
            self.view.removal = None;
            return;
        }
        let key = match removal {
            Removal::Subscription(_) => Key::RemoveSubscriptionConfirm,
            Removal::Profile(_) => Key::ProfileRemoved,
        };
        let mut confirm = false;
        let mut cancel = false;
        let width = modal_content_width(ui, 380.0, 32.0);
        egui::Window::new(self.tr(Key::Remove))
            .id(egui::Id::unique("remove-confirmation"))
            .collapsible(false)
            .resizable(false)
            .max_width(width)
            .show(ui.ctx(), |ui| {
                ui.add(egui::Label::new(self.tr(key)).wrap());
                ui.horizontal_wrapped(|ui| {
                    confirm = ui.button(self.tr(Key::ConfirmRemove)).clicked();
                    cancel = ui.button(self.tr(Key::Cancel)).clicked();
                });
            });
        if confirm {
            self.emit(Intent::ConfirmRemoval(removal));
        }
        if cancel {
            self.view.removal = None;
        }
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
