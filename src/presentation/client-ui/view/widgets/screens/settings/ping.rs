use vpn_presentation::localization::{self, Key, Locale};
use vpn_presentation::model::{ErrorNotice, Intent};
use vpn_presentation::session::{ProbeHttpMethod, ProbeMethod, ProbeSettings, ProbeStrategy};

use super::{ENTRY_WIDTH, ViewFrame, choice_row, divider, group, setting_row, toggle_row};
use crate::view::settings_state::{PingDisplay, ProbeSettingsDraft};
use crate::view::widgets::controls::{primary_action, roomy_input, selector, selector_option};

impl ViewFrame<'_> {
    pub(super) fn ping_settings(&mut self, ui: &mut egui::Ui) {
        let locale = self.view.locale;
        let ping = &mut self.view.desktop_settings.ping;
        let draft = ping
            .draft
            .get_or_insert_with(|| ProbeSettingsDraft::new(&self.snapshot.probe_settings));
        draft.synchronize(self.snapshot);
        let enabled = !self.snapshot.busy && draft.pending.is_none();
        let save = ui
            .add_enabled_ui(enabled, |ui| {
                probe_form(ui, locale, draft, self.snapshot.probe_icmp_available)
            })
            .inner;
        let update = save.then(|| {
            draft.outcome = None;
            Intent::SetProbeSettings(draft.value.clone())
        });
        probe_feedback(ui, locale, draft);
        group(ui, None, |ui| {
            choice_row(
                ui,
                locale,
                Key::PingDisplay,
                &mut ping.display_override,
                &[
                    (None, Key::Automatic),
                    (Some(PingDisplay::Time), Key::PingTime),
                    (Some(PingDisplay::Icon), Key::PingIcon),
                ],
            );
        });
        if let Some(intent) = update {
            self.emit(intent);
        }
    }
}

fn probe_form(
    ui: &mut egui::Ui,
    locale: Locale,
    draft: &mut ProbeSettingsDraft,
    icmp_available: bool,
) -> bool {
    let tr = |key| localization::text(locale, key);
    let previous = draft.value.clone();
    group(ui, None, |ui| {
        method_row(ui, locale, &mut draft.value.method, icmp_available);
    });
    ui.weak(tr(method_help(draft.value.method)));
    ui.add_space(8.0);
    let http_enabled = matches!(
        draft.value.method,
        ProbeMethod::Automatic | ProbeMethod::Proxy
    );
    ui.add_enabled_ui(http_enabled, |ui| {
        http_rows(ui, locale, draft);
    });
    measurement_rows(ui, locale, draft);
    if draft.value != previous {
        draft.outcome = None;
    }
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        let save = ui
            .add_enabled(draft.is_dirty(), primary_action(ui, tr(Key::Save)))
            .clicked();
        if draft.pending.is_some() {
            ui.spinner();
        }
        save
    })
    .inner
}

fn method_help(method: ProbeMethod) -> Key {
    match method {
        ProbeMethod::Automatic => Key::PingMethodHelp,
        ProbeMethod::Tcp => Key::PingTcpHelp,
        ProbeMethod::Icmp => Key::PingIcmpHelp,
        ProbeMethod::Proxy => Key::PingProxyHelp,
    }
}

fn method_row(ui: &mut egui::Ui, locale: Locale, method: &mut ProbeMethod, icmp_available: bool) {
    let tr = |key| localization::text(locale, key);
    let options = [
        (ProbeMethod::Automatic, Key::Automatic),
        (ProbeMethod::Tcp, Key::PingTcp),
        (ProbeMethod::Icmp, Key::PingIcmp),
        (ProbeMethod::Proxy, Key::PingProxy),
    ];
    let selected = options.iter().find(|(value, _)| value == method).unwrap().1;
    setting_row(ui, tr(Key::PingMethod), |ui| {
        selector(
            ui,
            Key::PingMethod.id(),
            super::CONTROL_WIDTH,
            tr(selected),
            |ui| {
                for (value, label) in options {
                    let enabled = value != ProbeMethod::Icmp || icmp_available;
                    let button = selector_option(ui, *method == value, tr(label));
                    if ui
                        .add_enabled(enabled, button)
                        .on_disabled_hover_text(tr(Key::PingIcmpUnavailable))
                        .clicked()
                    {
                        *method = value;
                        ui.close();
                    }
                }
            },
        );
    });
}

fn http_rows(ui: &mut egui::Ui, locale: Locale, draft: &mut ProbeSettingsDraft) {
    let tr = |key| localization::text(locale, key);
    group(ui, None, |ui| {
        choice_row(
            ui,
            locale,
            Key::PingHttpMethod,
            &mut draft.value.http_method,
            &[
                (ProbeHttpMethod::Get, Key::PingGet),
                (ProbeHttpMethod::Head, Key::PingHead),
            ],
        );
        divider(ui);
        setting_row(ui, tr(Key::PingUrl), |ui| {
            ui.add(
                roomy_input(&mut draft.value.url, ENTRY_WIDTH.min(ui.available_width()))
                    .id_salt(Key::PingUrl.id()),
            );
        });
    });
}

fn measurement_rows(ui: &mut egui::Ui, locale: Locale, draft: &mut ProbeSettingsDraft) {
    let tr = |key| localization::text(locale, key);
    group(ui, None, |ui| {
        setting_row(ui, tr(Key::PingTimeout), |ui| {
            ui.add(egui::DragValue::new(&mut draft.value.timeout_ms).range(250..=10_000));
        });
        divider(ui);
        setting_row(ui, tr(Key::PingAttempts), |ui| {
            ui.add(egui::DragValue::new(&mut draft.value.attempts).range(1..=5));
        });
        divider(ui);
        warmup_row(ui, locale, &mut draft.value);
        divider(ui);
        choice_row(
            ui,
            locale,
            Key::PingStrategy,
            &mut draft.value.strategy,
            &[
                (ProbeStrategy::Last, Key::PingLast),
                (ProbeStrategy::Minimum, Key::PingMinimum),
                (ProbeStrategy::Average, Key::PingAverage),
                (ProbeStrategy::Median, Key::PingMedian),
            ],
        );
    });
    let help = match warmup_state(&draft.value) {
        (_, false) if draft.value.method == ProbeMethod::Icmp => Key::PingIcmpWarmupHelp,
        (_, false) => Key::PingWarmupMandatory,
        _ => Key::PingWarmupHelp,
    };
    ui.weak(tr(help));
    ui.add_space(8.0);
}

fn warmup_state(settings: &ProbeSettings) -> (bool, bool) {
    match settings.method {
        ProbeMethod::Icmp => (false, false),
        ProbeMethod::Tcp => (settings.warmup, true),
        ProbeMethod::Automatic | ProbeMethod::Proxy => {
            let mandatory = ProbeSettings {
                warmup: false,
                ..settings.clone()
            }
            .http_warmup_required();
            (settings.http_warmup_required(), !mandatory)
        }
    }
}

fn warmup_row(ui: &mut egui::Ui, locale: Locale, settings: &mut ProbeSettings) {
    let (mut effective, editable) = warmup_state(settings);
    ui.add_enabled_ui(editable, |ui| {
        toggle_row(ui, locale, Key::PingWarmup, &mut effective);
    });
    if editable {
        settings.warmup = effective;
    }
}

fn probe_feedback(ui: &mut egui::Ui, locale: Locale, draft: &ProbeSettingsDraft) {
    if let Some(outcome) = draft.outcome {
        match outcome {
            Ok(()) => {
                ui.label(localization::text(locale, Key::PingSettingsSaved));
            }
            Err(error) => {
                let notice = ErrorNotice::Client(error);
                ui.label(notice.text_with_code(locale, notice.key()));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn https_warmup_is_checked_and_locked_without_changing_the_saved_draft() {
        let settings = ProbeSettings {
            warmup: false,
            ..ProbeSettings::default()
        };
        assert_eq!(warmup_state(&settings), (true, false));
        assert!(!settings.warmup);
        assert!(!ProbeSettingsDraft::new(&settings).is_dirty());
    }

    #[test]
    fn http_and_tcp_warmup_remain_optional_and_icmp_has_no_warmup_control() {
        let mut settings = ProbeSettings {
            url: "http://probe.example/check".into(),
            warmup: false,
            ..ProbeSettings::default()
        };
        assert_eq!(warmup_state(&settings), (false, true));
        settings.warmup = true;
        assert_eq!(warmup_state(&settings), (true, true));
        settings.method = ProbeMethod::Tcp;
        settings.url = "https://probe.example/check".into();
        settings.warmup = false;
        assert_eq!(warmup_state(&settings), (false, true));
        settings.method = ProbeMethod::Icmp;
        settings.warmup = true;
        assert_eq!(warmup_state(&settings), (false, false));
    }
}
