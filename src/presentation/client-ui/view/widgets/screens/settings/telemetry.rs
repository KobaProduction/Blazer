use vpn_presentation::localization::{self, Key, Locale};
use vpn_presentation::model::Intent;
use vpn_presentation::session::{TelemetryBearerChange, TelemetryMode, TelemetrySettingsSummary};

use super::{ENTRY_WIDTH, ViewFrame, choice_row, divider, group, setting_row};
use crate::view::view_model::TelemetrySettingsDraft;
use crate::view::widgets::controls::roomy_input;

impl ViewFrame<'_> {
    pub(super) fn telemetry_settings(&mut self, ui: &mut egui::Ui) {
        let locale = self.view.locale;
        let saved = &self.snapshot.telemetry_settings;
        let status = match (self.snapshot.telemetry_export_configured, saved.mode) {
            (true, TelemetryMode::Default) => Key::TelemetryDeploymentConfigured,
            (true, _) => Key::TelemetryConfigured,
            (false, _) => Key::TelemetryDisabled,
        };
        ui.weak(self.tr(status));
        ui.add_space(8.0);

        let draft = self
            .view
            .telemetry_settings
            .get_or_insert_with(|| TelemetrySettingsDraft::new(saved));
        draft.synchronize(self.snapshot);
        telemetry_feedback(ui, locale, draft);

        let enabled = !self.snapshot.busy && draft.pending.is_none();
        let save = ui.add_enabled_ui(enabled, |ui| telemetry_form(ui, locale, saved, draft));
        if save.inner {
            draft.outcome = None;
            let intent = telemetry_update(draft);
            self.emit(intent);
        }
    }
}

fn telemetry_feedback(ui: &mut egui::Ui, locale: Locale, draft: &TelemetrySettingsDraft) {
    let tr = |key| localization::text(locale, key);
    if draft.pending.is_some() && draft.mode == TelemetryMode::Manual {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(tr(Key::TelemetryChecking));
        });
        ui.add_space(8.0);
    } else if let Some(outcome) = draft.outcome {
        ui.label(tr(outcome));
        ui.add_space(8.0);
    }
}

fn telemetry_form(
    ui: &mut egui::Ui,
    locale: Locale,
    saved: &TelemetrySettingsSummary,
    draft: &mut TelemetrySettingsDraft,
) -> bool {
    let tr = |key| localization::text(locale, key);
    group(ui, None, |ui| {
        choice_row(
            ui,
            locale,
            Key::TelemetryExportMode,
            &mut draft.mode,
            &[
                (TelemetryMode::Disabled, Key::TelemetryDisabledMode),
                (TelemetryMode::Manual, Key::TelemetryManualMode),
            ],
        );
        if draft.mode == TelemetryMode::Manual {
            divider(ui);
            telemetry_endpoint(ui, locale, draft);
            divider(ui);
            telemetry_bearer(ui, locale, draft);
            if saved.has_bearer_token {
                ui.weak(tr(Key::TelemetryBearerConfigured));
                if ui
                    .checkbox(&mut draft.clear_bearer, tr(Key::TelemetryClearBearer))
                    .changed()
                    && draft.clear_bearer
                {
                    draft.bearer.clear();
                }
            }
        } else {
            draft.bearer.clear();
            draft.clear_bearer = false;
        }
    });

    let manual = draft.mode == TelemetryMode::Manual;
    let allowed = if manual {
        !draft.endpoint.trim().is_empty()
    } else {
        draft.mode != saved.mode
    };
    let action = if manual {
        Key::TelemetryCheckAndSave
    } else {
        Key::Save
    };
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.add_enabled(
            allowed,
            egui::Button::new(tr(action)).min_size(egui::vec2(124.0, 32.0)),
        )
        .clicked()
    })
    .inner
}

fn telemetry_endpoint(ui: &mut egui::Ui, locale: Locale, draft: &mut TelemetrySettingsDraft) {
    setting_row(
        ui,
        localization::text(locale, Key::TelemetryEndpoint),
        |ui| {
            ui.add(
                roomy_input(&mut draft.endpoint, ENTRY_WIDTH.min(ui.available_width()))
                    .id_salt(Key::TelemetryEndpoint.id()),
            );
        },
    );
}

fn telemetry_bearer(ui: &mut egui::Ui, locale: Locale, draft: &mut TelemetrySettingsDraft) {
    setting_row(ui, localization::text(locale, Key::TelemetryBearer), |ui| {
        ui.add_enabled_ui(!draft.clear_bearer, |ui| {
            bearer_field(ui, &mut draft.bearer);
        });
    });
}

fn bearer_field(ui: &mut egui::Ui, bearer: &mut String) -> egui::text_edit::TextEditOutput {
    let mut field = roomy_input(bearer, ENTRY_WIDTH.min(ui.available_width()))
        .id_salt(Key::TelemetryBearer.id())
        .password(true)
        .show(ui);
    field.state.clear_undoer();
    field.state.clone().store(ui.ctx(), field.response.id);
    field
}

fn telemetry_update(draft: &mut TelemetrySettingsDraft) -> Intent {
    let token = std::mem::take(&mut draft.bearer);
    let bearer = if std::mem::take(&mut draft.clear_bearer) {
        TelemetryBearerChange::Clear
    } else if token.is_empty() {
        TelemetryBearerChange::Keep
    } else {
        TelemetryBearerChange::Replace(token)
    };
    Intent::SetTelemetrySettings {
        mode: draft.mode,
        endpoint: draft.endpoint.trim_matches(' ').to_owned(),
        bearer,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bearer_field_accepts_paste_without_rendering_the_token() {
        let context = egui::Context::default();
        let mut bearer = String::new();
        let mut output = context.run_ui(egui::RawInput::default(), |ui| {
            bearer_field(ui, &mut bearer).response.request_focus();
        });
        output.textures_delta.clear();

        let sample = "sample-token";
        let mut output = context.run_ui(
            egui::RawInput {
                events: vec![egui::Event::Paste(sample.to_owned())],
                ..Default::default()
            },
            |ui| {
                let field = bearer_field(ui, &mut bearer);
                assert!(!field.galley.job.text.contains(sample));
            },
        );
        output.textures_delta.clear();
        assert_eq!(bearer, sample);
    }

    #[test]
    fn bearer_replacement_clears_the_edit_and_blank_submits_keep() {
        let mut draft = TelemetrySettingsDraft::new(&TelemetrySettingsSummary {
            mode: TelemetryMode::Manual,
            endpoint: Some("https://collector.example/v1/traces".to_owned()),
            has_bearer_token: true,
        });
        draft.bearer = "sample-token".to_owned();
        assert!(matches!(
            telemetry_update(&mut draft),
            Intent::SetTelemetrySettings {
                bearer: TelemetryBearerChange::Replace(token),
                ..
            } if token == "sample-token"
        ));
        assert!(draft.bearer.is_empty());
        assert!(matches!(
            telemetry_update(&mut draft),
            Intent::SetTelemetrySettings {
                bearer: TelemetryBearerChange::Keep,
                ..
            }
        ));
        draft.clear_bearer = true;
        assert!(matches!(
            telemetry_update(&mut draft),
            Intent::SetTelemetrySettings {
                bearer: TelemetryBearerChange::Clear,
                ..
            }
        ));
        assert!(!draft.clear_bearer);
    }
}
