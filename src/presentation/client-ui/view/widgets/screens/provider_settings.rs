use vpn_presentation::session::{
    ProviderChangeKind, ProviderChangePreview, ProviderSubscriptionSummary, SubscriptionSummary,
};

use super::ViewFrame;
use super::settings::group;
use crate::view::view_model::ProviderSettingsDraft;
use crate::view::widgets::controls::roomy_input;
use vpn_presentation::import;
use vpn_presentation::localization::{self, Key, Locale};
use vpn_presentation::model::Intent;

impl ViewFrame<'_> {
    pub(super) fn provider_settings(&mut self, ui: &mut egui::Ui) {
        let Some(draft) = &self.view.provider_settings else {
            return;
        };
        let snapshot = self.snapshot;
        let uuid = &draft.subscription_uuid;
        let Some(source) = snapshot
            .subscriptions
            .iter()
            .find(|source| source.subscription_uuid == *uuid)
        else {
            ui.label(self.tr(Key::SubscriptionMissing));
            return;
        };
        let provider = snapshot
            .provider_subscriptions
            .iter()
            .find(|provider| provider.subscription_uuid == *uuid);
        let address_blocked = self.subscription_change_blocked(uuid);
        let Some(draft) = &mut self.view.provider_settings else {
            return;
        };
        if draft
            .pending
            .zip(snapshot.completed_request)
            .is_some_and(|(pending, done)| pending.0 <= done.0)
        {
            draft.pending = None;
            if snapshot.last_error.is_none() {
                draft.title = None;
                draft.url = None;
            }
        }
        let enabled = !snapshot.busy && draft.pending.is_none();
        ui.heading(&source.title);
        ui.add_space(8.0);
        let editor = subscription_editor(
            ui,
            self.view.locale,
            source,
            draft,
            enabled,
            address_blocked,
        );
        let fallback = fallback_form(ui, self.view.locale, draft, provider, enabled);
        if let Some(intent) = editor.or(fallback) {
            self.emit(intent);
        }
        if let Some(provider) = provider {
            self.provider_status(ui, provider);
        }
    }

    fn provider_status(&mut self, ui: &mut egui::Ui, provider: &ProviderSubscriptionSummary) {
        if provider.last_update_used_fallback {
            ui.weak(self.tr(Key::FallbackUsed));
        }
        if provider.raw_config_hidden {
            ui.weak(self.tr(Key::ProfileHidden));
        }
        for pending in &provider.pending {
            ui.push_id(&pending.nonce, |ui| {
                self.provider_decision(ui, provider, pending)
            });
        }
    }

    fn provider_decision(
        &mut self,
        ui: &mut egui::Ui,
        provider: &ProviderSubscriptionSummary,
        pending: &ProviderChangePreview,
    ) {
        let title = match pending.kind {
            ProviderChangeKind::Fallback => Key::ProviderProposedFallback,
            ProviderChangeKind::Migration => Key::ProviderProposedMigration,
        };
        ui.add_space(8.0);
        group(ui, Some(self.tr(title)), |ui| {
            ui.label(self.message(Key::ProviderOrigin, &[("origin", &pending.from_origin)]));
            ui.label(self.message(Key::ProviderTarget, &[("origin", &pending.target_origin)]));
            ui.weak(self.tr(Key::ProviderApprovalHelp));
            ui.add_space(6.0);
            ui.add_enabled_ui(!self.snapshot.busy, |ui| {
                ui.horizontal_wrapped(|ui| {
                    if ui.button(self.tr(Key::AllowProviderChange)).clicked() {
                        self.emit(Intent::ApproveProviderChange {
                            subscription_uuid: provider.subscription_uuid.clone(),
                            nonce: pending.nonce.clone(),
                        });
                    }
                    if ui.button(self.tr(Key::RejectProviderChange)).clicked() {
                        self.emit(Intent::RejectProviderChange {
                            subscription_uuid: provider.subscription_uuid.clone(),
                            nonce: pending.nonce.clone(),
                        });
                    }
                });
            });
        });
    }
}

fn subscription_editor(
    ui: &mut egui::Ui,
    locale: Locale,
    source: &SubscriptionSummary,
    draft: &mut ProviderSettingsDraft,
    enabled: bool,
    address_blocked: Option<Key>,
) -> Option<Intent> {
    let tr = |key| localization::text(locale, key);
    let original_title = if source.auto_title {
        ""
    } else {
        source.title.as_str()
    };
    let title = draft.title.get_or_insert_with(|| original_title.to_owned());
    let saved_url = source.url.expose();
    let mut input = draft.url.take().unwrap_or_else(|| saved_url.to_owned());
    let mut intent = None;
    group(ui, None, |ui| {
        ui.weak(tr(Key::SubscriptionName));
        ui.add_enabled(
            enabled,
            roomy_input(title, ui.available_width())
                .id_salt("subscription-title")
                .hint_text(&source.title),
        );
        ui.weak(tr(Key::SubscriptionAutomaticName));
        ui.add_space(8.0);
        subscription_url_field(ui, locale, &mut input, &mut draft.url_visible, enabled);
        ui.add_space(6.0);
        let url = import::payload(&input);
        let url_changed = url != saved_url;
        let title = title.trim();
        let changed = title != original_title || url_changed;
        let writable = enabled && !url.is_empty() && (!url_changed || address_blocked.is_none());
        if url_changed && address_blocked == Some(Key::SubscriptionDisconnectFirst) {
            ui.weak(tr(Key::SubscriptionDisconnectFirst));
        }
        if ui
            .add_enabled(changed && writable, egui::Button::new(tr(Key::Save)))
            .clicked()
        {
            intent = Some(Intent::SaveSubscription {
                subscription_uuid: Some(source.subscription_uuid.clone()),
                title: title.to_owned(),
                url: if url_changed {
                    url.to_owned()
                } else {
                    String::new()
                },
            });
        }
    });
    draft.url = (import::payload(&input) != saved_url).then_some(input);
    intent
}

fn subscription_url_field(
    ui: &mut egui::Ui,
    locale: Locale,
    input: &mut String,
    visible: &mut bool,
    editable: bool,
) {
    let tr = |key| localization::text(locale, key);
    ui.weak(tr(Key::SubscriptionAddress));
    ui.add_enabled_ui(editable, |ui| {
        url_input(
            ui,
            input,
            "subscription-primary-url",
            tr(Key::SubscriptionUrlHint),
            !*visible,
        );
    });
    ui.horizontal_wrapped(|ui| {
        let key = if *visible {
            Key::HideSubscriptionUrl
        } else {
            Key::ShowSubscriptionUrl
        };
        if ui.button(tr(key)).clicked() {
            *visible = !*visible;
        }
        let url = import::payload(input);
        if ui
            .add_enabled(
                !url.is_empty(),
                egui::Button::new(tr(Key::CopySubscriptionUrl)),
            )
            .clicked()
        {
            ui.ctx().copy_text(url.to_owned());
        }
    });
}

fn url_input(ui: &mut egui::Ui, url: &mut String, id_salt: &str, hint: &str, masked: bool) {
    let mut field = roomy_input(url, ui.available_width())
        .id_salt(id_salt)
        .password(masked)
        .hint_text(hint)
        .show(ui);
    field.state.clear_undoer();
    field.state.store(ui.ctx(), field.response.id);
}

fn fallback_form(
    ui: &mut egui::Ui,
    locale: Locale,
    draft: &mut ProviderSettingsDraft,
    provider: Option<&ProviderSubscriptionSummary>,
    enabled: bool,
) -> Option<Intent> {
    let tr = |key| localization::text(locale, key);
    let configured = provider.is_some_and(|provider| provider.has_committed_fallback);
    let mut intent = None;
    group(ui, Some(tr(Key::FallbackUrl)), |ui| {
        let status = if configured {
            Key::FallbackConfigured
        } else {
            Key::FallbackNotSet
        };
        ui.weak(tr(status));
        ui.add_enabled_ui(enabled, |ui| {
            url_input(
                ui,
                &mut draft.fallback_url,
                "subscription-fallback-url",
                tr(Key::SubscriptionUrlHint),
                true,
            );
        });
        ui.horizontal_wrapped(|ui| {
            let valid = enabled && !draft.fallback_url.trim().is_empty();
            if ui
                .add_enabled(valid, egui::Button::new(tr(Key::Save)))
                .clicked()
            {
                intent = Some(Intent::SetFallback {
                    subscription_uuid: draft.subscription_uuid.clone(),
                    url: Some(std::mem::take(&mut draft.fallback_url).trim().to_owned()),
                });
            }
            if configured
                && ui
                    .add_enabled(enabled, egui::Button::new(tr(Key::RemoveFallback)))
                    .clicked()
            {
                intent = Some(Intent::SetFallback {
                    subscription_uuid: draft.subscription_uuid.clone(),
                    url: None,
                });
            }
        });
    });
    intent
}
