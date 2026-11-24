use super::{DIALOG_WIDTH, FIELD_HEIGHT, FormAction, ViewFrame, dialog_frame};
use crate::view::theme;
use crate::view::view_model::ProfileEditor;
use crate::view::widgets::controls::{primary_action, roomy_input, secondary_action};
use crate::view::widgets::layout::modal_content_width;
use vpn_presentation::import;
use vpn_presentation::localization::Key;
use vpn_presentation::model::{ErrorNotice, Intent, Removal, profile_details_hidden};
use vpn_presentation::session::{
    ProbeMethod, ProfileLatency, ProfileProbeState, ProfileSummary, Secret,
};

impl ViewFrame<'_> {
    pub(in crate::view::widgets) fn profile_editor_dialog(&mut self, ui: &mut egui::Ui) {
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
        if !pending && (modal.inner == FormAction::Close || modal.should_close()) {
            self.view.dismissed_error = draft.feedback.or(draft.probe_feedback);
            return;
        }
        if modal.inner == FormAction::Submit
            && let Some(intent) = editor_save_intent(profile, &draft)
        {
            self.emit(intent);
        }
        self.view.profile_editor = Some(draft);
    }

    fn profile_editor_contents(
        &mut self,
        ui: &mut egui::Ui,
        profile: &ProfileSummary,
        draft: &mut ProfileEditor,
    ) -> FormAction {
        ui.set_width(modal_content_width(ui, DIALOG_WIDTH, 56.0));
        ui.heading(self.tr(Key::EditConfiguration));
        ui.add_space(8.0);
        let height = (ui.ctx().content_rect().height() - 160.0).clamp(120.0, 560.0);
        egui::ScrollArea::vertical()
            .max_height(height)
            .min_scrolled_height(height)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                self.profile_editor_fields(ui, profile, draft, height);
            });
        ui.add_space(8.0);
        self.profile_editor_footer(ui, profile, draft)
    }

    fn profile_editor_fields(
        &mut self,
        ui: &mut egui::Ui,
        profile: &ProfileSummary,
        draft: &mut ProfileEditor,
        height: f32,
    ) {
        let writable =
            profile.subscription_uuid.is_none() && !profile_details_hidden(self.snapshot, profile);
        ui.weak(self.tr(Key::ConfigurationName));
        if ui
            .add_enabled(
                writable && draft.pending.is_none(),
                roomy_input(&mut draft.title, ui.available_width())
                    .id_salt(("profile-title", &draft.uuid)),
            )
            .changed()
        {
            draft.feedback = None;
        }
        if !writable {
            ui.add(egui::Label::new(self.tr(Key::ProfileSubscriptionReadOnly)).wrap());
        }
        ui.add_space(8.0);
        if profile_details_hidden(self.snapshot, profile) {
            draft.text.clear();
            ui.label(self.tr(Key::ProfileHidden));
        } else {
            let label = if profile.protocol == "AmneziaWG" {
                Key::AmneziaProfileParameters
            } else {
                Key::ProfileSource
            };
            ui.weak(self.tr(label));
            self.profile_source_field(
                ui,
                profile,
                draft,
                writable,
                (height - 180.0).clamp(80.0, 340.0),
            );
        }
        ui.add_space(8.0);
        self.profile_editor_ping(ui, profile, draft, writable);
        if let Some((_, error)) = draft.feedback {
            ui.add(egui::Label::new(error.text_with_code(self.view.locale, error.key())).wrap());
        }
    }

    fn profile_source_field(
        &self,
        ui: &mut egui::Ui,
        profile: &ProfileSummary,
        draft: &mut ProfileEditor,
        writable: bool,
        height: f32,
    ) {
        let editable = writable
            && self.snapshot.active_profile_uuid.as_ref() != Some(&draft.uuid)
            && draft.pending.is_none();
        egui::ScrollArea::both()
            .max_height(height)
            .min_scrolled_height(height)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let changed = if editable {
                    source_field(ui, &mut draft.text, &draft.uuid)
                } else {
                    let mut source = draft.text.as_str();
                    source_field(ui, &mut source, &draft.uuid)
                };
                if changed {
                    draft.feedback = None;
                }
            });
        if writable && self.snapshot.active_profile_uuid.as_ref() == Some(&profile.profile_uuid) {
            ui.weak(self.tr(Key::DisconnectFirst));
        }
    }

    fn profile_editor_ping(
        &mut self,
        ui: &mut egui::Ui,
        profile: &ProfileSummary,
        draft: &ProfileEditor,
        writable: bool,
    ) {
        let status = self.snapshot.profile_probes.get(&profile.profile_uuid);
        let pending = status.is_some_and(|status| {
            matches!(
                status.state,
                ProfileProbeState::Queued | ProfileProbeState::Running
            )
        });
        let dirty = writable && editor_source_changed(profile, draft);
        let blocked = if dirty {
            Some(Key::ProfileSaveBeforeProbe)
        } else if pending || draft.pending.is_some() {
            Some(Key::Working)
        } else {
            self.probe_blocked()
        };
        ui.allocate_ui_with_layout(
            egui::vec2(ui.available_width(), 70.0),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.horizontal(|ui| {
                    let response = ui.add_enabled(
                        blocked.is_none(),
                        primary_action(ui, self.tr(Key::PingTest)),
                    );
                    let response = if let Some(reason) = blocked {
                        response.on_disabled_hover_text(self.tr(reason))
                    } else {
                        response
                    };
                    if response.clicked() {
                        self.emit(Intent::ProbeProfile(profile.profile_uuid.clone()));
                    }
                    let (slot, _) = ui
                        .allocate_exact_size(egui::vec2(18.0, FIELD_HEIGHT), egui::Sense::hover());
                    if pending {
                        egui::Spinner::new().paint_at(ui, slot.shrink(2.0));
                    }
                    if let Some(status) = status {
                        let key = match status.state {
                            ProfileProbeState::Queued => Key::PingQueued,
                            ProfileProbeState::Running => Key::PingRunning,
                            ProfileProbeState::Finished => Key::PingFinished,
                        };
                        ui.weak(self.tr(key));
                        let method = match status.method {
                            ProbeMethod::Automatic => Key::Automatic,
                            ProbeMethod::Tcp => Key::PingTcp,
                            ProbeMethod::Icmp => Key::PingIcmp,
                            ProbeMethod::Proxy => Key::PingProxy,
                        };
                        ui.weak(self.tr(method));
                    }
                });
                let rejected = self.editor_probe_error(draft);
                let message = self.editor_ping_message(profile, dirty, pending, rejected);
                let failed = rejected.is_some()
                    || (!pending
                        && self
                            .snapshot
                            .profile_latencies
                            .get(&profile.profile_uuid)
                            .is_some_and(|latency| {
                                !matches!(latency, ProfileLatency::Reachable { .. })
                            }));
                let color = if failed {
                    theme::ping_failure_color(ui.visuals())
                } else {
                    ui.visuals().weak_text_color()
                };
                ui.add(egui::Label::new(egui::RichText::new(&message).color(color)).truncate())
                    .on_hover_text(message);
            },
        );
    }

    fn editor_probe_error(&self, draft: &ProfileEditor) -> Option<ErrorNotice> {
        if let Some((_, error)) = draft.probe_feedback {
            return Some(error);
        }
        let request = draft.probe_request?;
        self.snapshot
            .probe_completions
            .iter()
            .find(|completion| completion.request_id == request)?
            .error
            .map(ErrorNotice::Client)
    }

    fn editor_ping_message(
        &self,
        profile: &ProfileSummary,
        dirty: bool,
        pending: bool,
        rejected: Option<ErrorNotice>,
    ) -> String {
        if dirty {
            return self.tr(Key::ProfileSaveBeforeProbe).to_owned();
        }
        if pending {
            return self.tr(Key::ProfileProbeSettings).to_owned();
        }
        if let Some(notice) = rejected {
            return notice
                .text_with_code(self.view.locale, notice.key())
                .into_owned();
        }
        self.snapshot
            .profile_latencies
            .get(&profile.profile_uuid)
            .map_or_else(
                || self.tr(Key::ProfileProbeSettings).to_owned(),
                |latency| self.ping_status(*latency),
            )
    }

    fn profile_editor_footer(
        &mut self,
        ui: &mut egui::Ui,
        profile: &ProfileSummary,
        draft: &ProfileEditor,
    ) -> FormAction {
        let active = self.snapshot.active_profile_uuid.as_ref() == Some(&draft.uuid);
        let writable =
            profile.subscription_uuid.is_none() && !profile_details_hidden(self.snapshot, profile);
        let idle = !self.snapshot.busy && draft.pending.is_none();
        let save = writable
            && idle
            && !draft.title.trim().is_empty()
            && (!active || !editor_source_changed(profile, draft))
            && (editor_source_changed(profile, draft) || draft.title.trim() != profile.name);
        ui.with_layout(
            egui::Layout::right_to_left(egui::Align::Center).with_main_wrap(true),
            |ui| {
                if ui
                    .add_enabled(save, primary_action(ui, self.tr(Key::Save)))
                    .clicked()
                {
                    return FormAction::Submit;
                }
                if ui
                    .add_enabled(
                        draft.pending.is_none(),
                        secondary_action(ui, self.tr(Key::Close)),
                    )
                    .clicked()
                {
                    return FormAction::Close;
                }
                if ui
                    .add_enabled(idle && !active, secondary_action(ui, self.tr(Key::Remove)))
                    .clicked()
                {
                    self.view.removal = Some(Removal::Profile(draft.uuid.clone()));
                    return FormAction::Close;
                }
                FormAction::None
            },
        )
        .inner
    }
}

fn source_field(ui: &mut egui::Ui, source: &mut dyn egui::TextBuffer, uuid: &str) -> bool {
    ui.add(
        egui::TextEdit::multiline(source)
            .id_salt(("profile-source", uuid))
            .code_editor()
            .desired_rows(8)
            .desired_width(ui.available_width())
            .char_limit(import::MAX_TEXT_BYTES),
    )
    .changed()
}

fn editor_source_changed(profile: &ProfileSummary, draft: &ProfileEditor) -> bool {
    profile
        .source
        .as_ref()
        .map_or(!import::payload(&draft.text).is_empty(), |source| {
            import::payload(source.expose()) != import::payload(&draft.text)
        })
}

fn editor_save_intent(profile: &ProfileSummary, draft: &ProfileEditor) -> Option<Intent> {
    if editor_source_changed(profile, draft) {
        Some(Intent::UpdateProfile {
            profile_uuid: draft.uuid.clone(),
            text: Secret::new(import::payload(&draft.text)),
            title: Some(draft.title.trim().to_owned()),
        })
    } else if draft.title.trim() != profile.name {
        Some(Intent::RenameProfile {
            profile_uuid: draft.uuid.clone(),
            title: draft.title.trim().to_owned(),
        })
    } else {
        None
    }
}
