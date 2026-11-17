use vpn_presentation::localization::{self, Key, Locale};
use vpn_presentation::model::Intent;
use vpn_presentation::session::{
    ConnectionMode, RouteAction, RouteRule, RouteTarget, RoutingAssignment, RoutingProfile,
    RoutingSelection,
};

use super::{
    GROUP_GAP, ROW_HEIGHT, ViewFrame, choice_row, divider, group, link_row, row_note, setting_row,
    text_row, toggle_row,
};
use crate::view::settings_state::{DnsType, RoutingDialog, RoutingProfileDraft};
use crate::view::view_model::{Screen, SettingsPage};
use crate::view::widgets::controls::{roomy_input, select_value, selector};

const ROUTING_DIALOG_WIDTH: f32 = 360.0;
const PROFILE_TITLE_LIMIT: usize = 127;
const DETAIL_BUTTON_WIDTH: f32 = 28.0;
const PREVIEW_GEOIP: &str =
    "https://github.com/Loyalsoldier/v2ray-rules-dat/releases/latest/download/geoip.dat";
const PREVIEW_GEOSITE: &str =
    "https://github.com/Loyalsoldier/v2ray-rules-dat/releases/latest/download/geosite.dat";

impl<'a> ViewFrame<'a> {
    fn routing_writable(&self) -> bool {
        self.snapshot.routing.local_proxy_available
            && !self.snapshot.busy
            && !self.view.preview
            && !self.view.routing_reconnect.awaiting_edit()
            && self.view.desktop_settings.routing.pending.is_none()
    }

    pub(in crate::view::widgets) fn routing_actions(
        &mut self,
        ui: &mut egui::Ui,
        page: SettingsPage,
    ) {
        ui.menu_button("…", |ui| match page {
            SettingsPage::Routing => self.routing_menu(ui, false),
            SettingsPage::SubscriptionRouting => self.routing_menu(ui, true),
            SettingsPage::RoutingProfile => self.routing_profile_menu(ui),
            _ => {}
        });
    }

    fn routing_menu(&mut self, ui: &mut egui::Ui, from_subscription: bool) {
        let enabled = self.routing_writable();
        if ui
            .add_enabled(enabled, egui::Button::new(self.tr(Key::RoutingAddProfile)))
            .clicked()
        {
            self.view.desktop_settings.routing.dialog = Some(if from_subscription {
                RoutingDialog::Title {
                    subscription_uuid: self.view.subscription_routing_uuid.clone(),
                    title: String::new(),
                }
            } else {
                RoutingDialog::ChooseGroup {
                    subscription_uuid: None,
                }
            });
            ui.close();
        }
        unavailable_menu_item(ui, self.view.locale, Key::RoutingImportClipboard);
        if from_subscription {
            let can_copy = self.copyable_subscriptions().next().is_some();
            if ui
                .add_enabled(
                    enabled && can_copy,
                    egui::Button::new(self.tr(Key::RoutingCopyOther)),
                )
                .clicked()
            {
                self.view.desktop_settings.routing.dialog =
                    Some(RoutingDialog::CopyFromSubscription(None));
                ui.close();
            }
            unavailable_menu_item(ui, self.view.locale, Key::RoutingExportClipboard);
        } else {
            unavailable_menu_item(ui, self.view.locale, Key::RoutingImportQr);
        }
    }

    fn routing_profile_menu(&mut self, ui: &mut egui::Ui) {
        unavailable_menu_item(ui, self.view.locale, Key::RoutingExportClipboard);
        let Some(profile) = self.selected_routing_profile() else {
            return;
        };
        let enabled = self.routing_writable();
        if ui
            .add_enabled(enabled, egui::Button::new(self.tr(Key::RoutingCopyProfile)))
            .clicked()
        {
            self.open_routing_copy(profile, self.view.desktop_settings.routing.profile_parent());
            ui.close();
            return;
        }
        if ui
            .add_enabled(enabled, egui::Button::new(self.tr(Key::Remove)))
            .clicked()
        {
            self.view.desktop_settings.routing.dialog = Some(RoutingDialog::Remove {
                profile_uuid: profile.uuid.clone(),
                title: profile.input.title.clone(),
            });
            ui.close();
        }
    }

    pub(super) fn routing_settings(&mut self, ui: &mut egui::Ui) {
        ui.label(self.tr(Key::RoutingExplanation));
        self.routing_status(ui);
        self.default_routing_profile(ui);
        self.routing_profiles(ui);
        self.routing_subscriptions(ui);
    }

    fn routing_status(&mut self, ui: &mut egui::Ui) {
        let routing = &self.snapshot.routing;
        let active = self.snapshot.active_mode == Some(ConnectionMode::LocalProxy);
        let saved = if active {
            routing.saved_for_active.as_ref()
        } else {
            routing.saved.as_ref()
        };
        ui.weak(self.tr(Key::RoutingLocalProxyOnly));
        group(ui, None, |ui| {
            setting_row(ui, self.tr(Key::RoutingSavedProfile), |ui| {
                ui.add(egui::Label::new(self.routing_revision_label(saved)).truncate());
            });
            divider(ui);
            setting_row(ui, self.tr(Key::RoutingActiveProfile), |ui| {
                let label = if active {
                    self.routing_revision_label(routing.active.as_ref())
                } else {
                    "—".to_owned()
                };
                ui.add(egui::Label::new(label).truncate());
            });
        });
        if let Some(pending) = &routing.pending_reconnect {
            ui.weak(self.tr(Key::RoutingPendingReconnect));
            if ui
                .add_enabled(
                    self.routing_writable(),
                    egui::Button::new(self.tr(Key::RoutingApplyNow)),
                )
                .clicked()
            {
                self.view.routing_reconnect.confirmation = Some(pending.clone());
            }
        }
    }

    pub(in crate::view::widgets) fn routing_reconnect_dialog(&mut self, ui: &egui::Ui) {
        let Some(pending) = self.view.routing_reconnect.confirmation.take() else {
            return;
        };
        let modal = egui::Modal::new(egui::Id::unique("routing-reconnect")).show(ui.ctx(), |ui| {
            ui.set_width(routing_dialog_width(ui));
            ui.heading(self.tr(Key::RoutingConfirmReconnect));
            ui.label(self.tr(Key::RoutingPendingReconnect));
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                let apply = ui
                    .add_enabled(
                        self.routing_writable(),
                        egui::Button::new(self.tr(Key::RoutingApplyNow)),
                    )
                    .clicked();
                let defer = ui
                    .button(self.tr(Key::RoutingApplyNextConnection))
                    .clicked();
                (apply, defer)
            })
            .inner
        });
        let (apply, defer) = modal.inner;
        if apply {
            self.emit(Intent::ReconnectRouting(pending));
        } else if !defer && !modal.should_close() {
            self.view.routing_reconnect.confirmation = Some(pending);
        }
    }

    fn routing_revision_label(&self, selection: Option<&RoutingSelection>) -> String {
        let Some(selection) = selection else {
            return self.tr(Key::RoutingDefaultProfile).to_owned();
        };
        let title = self
            .routing_profile_by_uuid(&selection.profile_uuid)
            .map_or(self.tr(Key::RoutingProfile), |profile| {
                profile.input.title.as_str()
            });
        let revision = selection.revision.to_string();
        self.message(
            Key::RoutingRevisionLabel,
            &[("title", title), ("revision", &revision)],
        )
    }

    fn default_routing_profile(&mut self, ui: &mut egui::Ui) {
        let profiles = &self.snapshot.routing.catalog.profiles;
        let current = &self.snapshot.routing.catalog.default_profile_uuid;
        let mut selected = current.clone();
        let locale = self.view.locale;
        group(ui, Some(self.tr(Key::RoutingBundled)), |ui| {
            if link_row(ui, self.tr(Key::RoutingDefaultProfile)) {
                self.open_routing_editor(None, false);
            }
            divider(ui);
            ui.add_enabled_ui(self.routing_writable(), |ui| {
                assignment_choice(
                    ui,
                    locale,
                    profiles,
                    &mut selected,
                    Key::RoutingDefaultProfile,
                );
            });
        });
        if &selected != current {
            self.emit(Intent::SetRoutingAssignment {
                subscription_uuid: None,
                assignment: RoutingAssignment {
                    enabled: selected.is_some(),
                    profile_uuid: selected,
                    ignore_provider: false,
                },
            });
        }
    }

    fn routing_profiles(&mut self, ui: &mut egui::Ui) {
        let mut open = None;
        let catalog = &self.snapshot.routing.catalog;
        group(ui, Some(self.tr(Key::RoutingManualGroup)), |ui| {
            let profiles = catalog.profiles.iter().filter(|profile| {
                !catalog
                    .assignments
                    .values()
                    .any(|assignment| assignment.profile_uuid.as_deref() == Some(&profile.uuid))
            });
            let mut any = false;
            for (index, profile) in profiles.enumerate() {
                any = true;
                if index != 0 {
                    divider(ui);
                }
                if ui
                    .push_id(&profile.uuid, |ui| link_row(ui, &profile.input.title))
                    .inner
                {
                    open = Some(profile);
                }
            }
            if !any {
                row_note(ui, self.tr(Key::RoutingNoRules));
            }
        });
        if let Some(profile) = open {
            self.open_routing_editor(Some(RoutingProfileDraft::from_profile(profile)), false);
        }
    }

    fn routing_subscriptions(&mut self, ui: &mut egui::Ui) {
        for source in &self.snapshot.subscriptions {
            let uuid = &source.subscription_uuid;
            let catalog = &self.snapshot.routing.catalog;
            let profile = catalog
                .assignments
                .get(uuid)
                .and_then(|assignment| assignment.profile_uuid.as_deref())
                .and_then(|profile_uuid| catalog.lookup(profile_uuid));
            ui.push_id(uuid, |ui| {
                let (mut open_context, mut open_profile) = (false, false);
                group(ui, Some(&source.title), |ui| {
                    open_context = link_row(ui, self.tr(Key::Routing));
                    if let Some(profile) = profile {
                        divider(ui);
                        open_profile = link_row(ui, &profile.input.title);
                    }
                });
                if open_context || open_profile {
                    self.view.open_subscription_routing(uuid);
                }
                if open_profile && let Some(profile) = profile {
                    self.open_routing_editor(
                        Some(RoutingProfileDraft::from_profile(profile)),
                        true,
                    );
                }
            });
        }
    }

    pub(super) fn subscription_routing(&mut self, ui: &mut egui::Ui) {
        let Some(source) = self.snapshot.subscriptions.iter().find(|source| {
            self.view.subscription_routing_uuid.as_deref() == Some(&source.subscription_uuid)
        }) else {
            ui.weak(self.tr(Key::SubscriptionMissing));
            return;
        };
        let uuid = &source.subscription_uuid;
        let catalog = &self.snapshot.routing.catalog;
        let current = catalog.assignments.get(uuid).cloned().unwrap_or_default();
        let mut assignment = current.clone();
        self.subscription_routing_options(ui, &source.title, &mut assignment);
        let mut open = None;
        group(ui, Some(self.tr(Key::RoutingRuleSelection)), |ui| {
            let writable = self.routing_writable();
            if subscription_rule(
                ui,
                self.tr(Key::RoutingUseGlobal),
                None,
                &mut assignment.profile_uuid,
                writable,
            ) {
                open = Some(
                    catalog
                        .default_profile_uuid
                        .as_deref()
                        .and_then(|uuid| catalog.lookup(uuid)),
                );
            }
            for profile in &catalog.profiles {
                divider(ui);
                let label = profile.input.title.as_str();
                if subscription_rule(
                    ui,
                    label,
                    Some(&profile.uuid),
                    &mut assignment.profile_uuid,
                    writable,
                ) {
                    open = Some(Some(profile));
                }
            }
        });
        if assignment != current {
            self.emit(Intent::SetRoutingAssignment {
                subscription_uuid: Some(uuid.clone()),
                assignment,
            });
        }
        if let Some(profile) = open {
            self.open_routing_editor(profile.map(RoutingProfileDraft::from_profile), true);
        }
    }

    fn subscription_routing_options(
        &self,
        ui: &mut egui::Ui,
        title: &str,
        assignment: &mut RoutingAssignment,
    ) {
        let locale = self.view.locale;
        group(ui, Some(self.tr(Key::RoutingGeneral)), |ui| {
            setting_row(ui, self.tr(Key::SubscriptionName), |ui| {
                ui.add(egui::Label::new(title).truncate());
            });
            divider(ui);
            ui.add_enabled_ui(self.routing_writable(), |ui| {
                toggle_row(ui, locale, Key::RoutingEnabled, &mut assignment.enabled);
            });
            divider(ui);
            ui.add_enabled_ui(false, |ui| {
                toggle_row(
                    ui,
                    locale,
                    Key::RoutingIgnoreImport,
                    &mut assignment.ignore_provider,
                );
            })
            .response
            .on_disabled_hover_text(self.tr(Key::SettingUnavailable));
        });
    }

    fn routing_profile_by_uuid(&self, uuid: &str) -> Option<&'a RoutingProfile> {
        self.snapshot.routing.catalog.lookup(uuid)
    }

    fn selected_routing_profile(&self) -> Option<&'a RoutingProfile> {
        let uuid = self
            .view
            .desktop_settings
            .routing
            .draft
            .as_ref()?
            .profile_uuid
            .as_deref()?;
        self.routing_profile_by_uuid(uuid)
    }

    fn open_routing_editor(&mut self, draft: Option<RoutingProfileDraft>, from_subscription: bool) {
        let settings = &mut self.view.desktop_settings.routing;
        if settings.pending.is_some() {
            return;
        }
        settings.open_profile(draft, from_subscription);
        self.view
            .open(Screen::Settings(SettingsPage::RoutingProfile));
    }

    fn open_routing_copy(&mut self, profile: &RoutingProfile, parent: SettingsPage) {
        let mut draft = RoutingProfileDraft::from_input(&profile.input);
        draft.title = self
            .message(Key::RoutingCopyTitle, &[("title", &profile.input.title)])
            .chars()
            .take(PROFILE_TITLE_LIMIT)
            .collect();
        self.open_routing_editor(Some(draft), parent == SettingsPage::SubscriptionRouting);
    }

    pub(super) fn routing_profile(&mut self, ui: &mut egui::Ui) {
        let enabled = self.routing_writable();
        let locale = self.view.locale;
        let settings = &mut self.view.desktop_settings.routing;
        let Some(draft) = settings.draft.as_mut() else {
            built_in_profile(ui, locale, self.view.preview);
            return;
        };
        let save = profile_editor(ui, locale, draft, enabled);
        if save {
            let intent = draft.input().map(|profile| Intent::SaveRoutingProfile {
                uuid: draft.profile_uuid.clone(),
                profile,
            });
            settings.invalid_dns_endpoint = intent.is_none();
            if let Some(intent) = intent {
                self.emit(intent);
                return;
            }
        }
        if settings.invalid_dns_endpoint {
            ui.weak(localization::text(locale, Key::RoutingDnsEndpointInvalid));
        }
    }

    pub(super) fn routing_dialog(&mut self, ui: &mut egui::Ui) {
        let Some(dialog) = self.view.desktop_settings.routing.dialog.take() else {
            return;
        };
        self.view.desktop_settings.routing.dialog = match dialog {
            RoutingDialog::ChooseGroup { subscription_uuid } => {
                self.routing_destination_dialog(ui, subscription_uuid)
            }
            RoutingDialog::Title {
                subscription_uuid,
                title,
            } => self.routing_title_dialog(ui, subscription_uuid, title),
            RoutingDialog::CopyFromSubscription(source) => self.copy_routing_dialog(ui, source),
            RoutingDialog::Remove {
                profile_uuid,
                title,
            } => self.remove_routing_dialog(ui, profile_uuid, title),
        };
    }

    fn routing_destination_dialog(
        &self,
        ui: &mut egui::Ui,
        mut subscription_uuid: Option<String>,
    ) -> Option<RoutingDialog> {
        let modal =
            egui::Modal::new(ui.make_persistent_id("routing-destination")).show(ui.ctx(), |ui| {
                ui.set_width(routing_dialog_width(ui));
                ui.heading(self.tr(Key::RoutingEditGroup));
                let source = self.snapshot.subscriptions.iter().find(|source| {
                    Some(source.subscription_uuid.as_str()) == subscription_uuid.as_deref()
                });
                let label = source.map_or(self.tr(Key::RoutingManualGroup), |source| {
                    source.title.as_str()
                });
                selector(
                    ui,
                    "routing-destination-choice",
                    ui.available_width(),
                    label,
                    |ui| {
                        select_value(
                            ui,
                            &mut subscription_uuid,
                            None,
                            self.tr(Key::RoutingManualGroup),
                        );
                        for source in &self.snapshot.subscriptions {
                            select_value(
                                ui,
                                &mut subscription_uuid,
                                Some(source.subscription_uuid.clone()),
                                &source.title,
                            );
                        }
                    },
                );
                dialog_buttons(
                    ui,
                    self.view.locale,
                    Key::RoutingNext,
                    self.routing_writable(),
                )
            });
        if modal.inner.confirm {
            Some(RoutingDialog::Title {
                subscription_uuid,
                title: String::new(),
            })
        } else {
            (!modal.inner.cancel && !modal.should_close())
                .then_some(RoutingDialog::ChooseGroup { subscription_uuid })
        }
    }

    fn routing_title_dialog(
        &mut self,
        ui: &mut egui::Ui,
        subscription_uuid: Option<String>,
        mut title: String,
    ) -> Option<RoutingDialog> {
        let modal = egui::Modal::new(ui.make_persistent_id("routing-title")).show(ui.ctx(), |ui| {
            ui.set_width(routing_dialog_width(ui));
            ui.heading(self.tr(Key::RoutingProfileTitle));
            ui.add(
                roomy_input(&mut title, ui.available_width())
                    .id_salt("new-routing-profile-title")
                    .char_limit(PROFILE_TITLE_LIMIT),
            );
            dialog_buttons(
                ui,
                self.view.locale,
                Key::RoutingCreate,
                self.routing_writable() && !title.trim().is_empty(),
            )
        });
        if modal.inner.confirm {
            let from_subscription = subscription_uuid.is_some();
            if let Some(uuid) = &subscription_uuid {
                self.view.open_subscription_routing(uuid);
            }
            self.open_routing_editor(
                Some(RoutingProfileDraft {
                    title,
                    ..Default::default()
                }),
                from_subscription,
            );
            return None;
        }
        (!modal.inner.cancel && !modal.should_close()).then_some(RoutingDialog::Title {
            subscription_uuid,
            title,
        })
    }

    fn copyable_subscriptions(&self) -> impl Iterator<Item = (&str, &str, &RoutingProfile)> {
        self.snapshot
            .subscriptions
            .iter()
            .filter_map(|subscription| {
                let uuid = subscription.subscription_uuid.as_str();
                if self.view.subscription_routing_uuid.as_deref() == Some(uuid) {
                    return None;
                }
                let assignment = self.snapshot.routing.catalog.assignments.get(uuid)?;
                let profile = self.routing_profile_by_uuid(assignment.profile_uuid.as_deref()?)?;
                Some((uuid, subscription.title.as_str(), profile))
            })
    }

    fn copy_routing_dialog(
        &mut self,
        ui: &mut egui::Ui,
        mut source_uuid: Option<String>,
    ) -> Option<RoutingDialog> {
        let modal = egui::Modal::new(ui.make_persistent_id("routing-copy")).show(ui.ctx(), |ui| {
            ui.set_width(routing_dialog_width(ui));
            ui.heading(self.tr(Key::RoutingCopyOther));
            let selected = self
                .copyable_subscriptions()
                .find(|(uuid, _, _)| Some(*uuid) == source_uuid.as_deref());
            let label = selected.map_or(self.tr(Key::Select), |(_, title, _)| title);
            selector(
                ui,
                "routing-copy-source",
                ui.available_width(),
                label,
                |ui| {
                    for (uuid, title, _) in self.copyable_subscriptions() {
                        select_value(ui, &mut source_uuid, Some(uuid.to_owned()), title);
                    }
                },
            );
            dialog_buttons(
                ui,
                self.view.locale,
                Key::RoutingCopyOther,
                self.routing_writable() && source_uuid.is_some(),
            )
        });
        if modal.inner.confirm {
            let profile = self
                .copyable_subscriptions()
                .find(|(uuid, _, _)| Some(*uuid) == source_uuid.as_deref())
                .map(|(_, _, profile)| profile.clone());
            if let Some(profile) = profile {
                self.open_routing_copy(&profile, SettingsPage::SubscriptionRouting);
                return None;
            }
        }
        (!modal.inner.cancel && !modal.should_close())
            .then_some(RoutingDialog::CopyFromSubscription(source_uuid))
    }

    fn remove_routing_dialog(
        &mut self,
        ui: &mut egui::Ui,
        profile_uuid: String,
        title: String,
    ) -> Option<RoutingDialog> {
        let modal =
            egui::Modal::new(ui.make_persistent_id("routing-remove")).show(ui.ctx(), |ui| {
                ui.set_width(routing_dialog_width(ui));
                ui.heading(self.tr(Key::RoutingRemoveTitle));
                ui.label(&title);
                dialog_buttons(
                    ui,
                    self.view.locale,
                    Key::ConfirmRemove,
                    self.routing_writable(),
                )
            });
        if modal.inner.confirm {
            self.emit(Intent::RemoveRoutingProfile { profile_uuid });
            return None;
        }
        (!modal.inner.cancel && !modal.should_close()).then_some(RoutingDialog::Remove {
            profile_uuid,
            title,
        })
    }
}

fn subscription_rule(
    ui: &mut egui::Ui,
    title: &str,
    uuid: Option<&str>,
    selected: &mut Option<String>,
    writable: bool,
) -> bool {
    ui.push_id(uuid, |ui| {
        ui.horizontal(|ui| {
            let label_width =
                (ui.available_width() - DETAIL_BUTTON_WIDTH - ui.spacing().item_spacing.x).max(0.0);
            ui.allocate_ui_with_layout(
                egui::vec2(label_width, ROW_HEIGHT),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                    let radio = egui::RadioButton::new(selected.as_deref() == uuid, title);
                    if ui.add_enabled(writable, radio).clicked() {
                        *selected = uuid.map(str::to_owned);
                    }
                },
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.button("›").on_hover_text(title).clicked()
            })
            .inner
        })
        .inner
    })
    .inner
}

fn assignment_choice(
    ui: &mut egui::Ui,
    locale: Locale,
    profiles: &[RoutingProfile],
    selected: &mut Option<String>,
    fallback_key: Key,
) {
    let fallback = localization::text(locale, fallback_key);
    let title = profiles
        .iter()
        .find(|profile| Some(profile.uuid.as_str()) == selected.as_deref())
        .map_or(fallback, |profile| profile.input.title.as_str());
    setting_row(ui, localization::text(locale, Key::RoutingProfile), |ui| {
        selector(
            ui,
            "routing-assignment",
            super::CONTROL_WIDTH,
            title,
            |ui| {
                select_value(ui, selected, None, fallback);
                for profile in profiles {
                    select_value(
                        ui,
                        selected,
                        Some(profile.uuid.clone()),
                        &profile.input.title,
                    );
                }
            },
        );
    });
}

fn built_in_profile(ui: &mut egui::Ui, locale: Locale, preview: bool) {
    let tr = |key| localization::text(locale, key);
    ui.label(tr(Key::RoutingBuiltInFallback));
    ui.add_space(GROUP_GAP);
    group(ui, None, |ui| {
        value_label(
            ui,
            tr(Key::RoutingProfileTitle),
            tr(Key::RoutingDefaultProfile),
        );
    });
    geo_card(ui, locale, "geoip", preview.then_some(PREVIEW_GEOIP));
    geo_card(ui, locale, "geosite", preview.then_some(PREVIEW_GEOSITE));
    group(ui, Some(tr(Key::RoutingDnsSettings)), |ui| {
        for (index, (key, example)) in [
            (Key::RoutingDnsType, "DoH"),
            (
                Key::RoutingDnsDomain,
                "https://cloudflare-dns.com/dns-query",
            ),
            (Key::RoutingDnsIp, "1.1.1.1"),
        ]
        .into_iter()
        .enumerate()
        {
            if index != 0 {
                divider(ui);
            }
            value_label(ui, tr(key), if preview { example } else { "—" });
        }
    });
}

fn value_label(ui: &mut egui::Ui, title: &str, value: &str) {
    setting_row(ui, title, |ui| {
        ui.add(egui::Label::new(value).truncate())
            .on_hover_text(value);
    });
}

fn geo_card(ui: &mut egui::Ui, locale: Locale, name: &str, source: Option<&str>) {
    let tr = |key| localization::text(locale, key);
    ui.push_id(name, |ui| {
        group(ui, Some(name), |ui| {
            value_label(ui, tr(Key::RoutingSource), source.unwrap_or("—"));
            divider(ui);
            setting_row(ui, tr(Key::RoutingLastUpdated), |ui| {
                unavailable_menu_item(ui, locale, Key::Refresh);
                ui.weak("—");
            });
        });
    });
}

fn profile_editor(
    ui: &mut egui::Ui,
    locale: Locale,
    draft: &mut RoutingProfileDraft,
    enabled: bool,
) -> bool {
    ui.add_enabled_ui(enabled, |ui| {
        group(ui, None, |ui| {
            setting_row(
                ui,
                localization::text(locale, Key::RoutingProfileTitle),
                |ui| {
                    ui.add(
                        roomy_input(
                            &mut draft.title,
                            super::ENTRY_WIDTH.min(ui.available_width()),
                        )
                        .id_salt(Key::RoutingProfileTitle.id())
                        .char_limit(PROFILE_TITLE_LIMIT),
                    );
                },
            );
        });
        geo_card(ui, locale, "geoip", None);
        geo_card(ui, locale, "geosite", None);
        dns_editor(ui, locale, draft);
        group(ui, None, |ui| {
            value_label(
                ui,
                localization::text(locale, Key::RoutingDomainStrategy),
                "—",
            );
        });
        rule_editor(ui, locale, &mut draft.rules);
        ui.add_enabled(
            !draft.title.trim().is_empty(),
            egui::Button::new(localization::text(locale, Key::Save)),
        )
        .clicked()
    })
    .inner
}

fn dns_editor(ui: &mut egui::Ui, locale: Locale, draft: &mut RoutingProfileDraft) {
    group(
        ui,
        Some(localization::text(locale, Key::RoutingDnsSettings)),
        |ui| {
            choice_row(
                ui,
                locale,
                Key::RoutingDnsType,
                &mut draft.dns_type,
                &[
                    (DnsType::Automatic, Key::Automatic),
                    (DnsType::System, Key::RoutingDnsSystem),
                    (DnsType::Tcp, Key::RoutingDnsTcp),
                    (DnsType::Tls, Key::RoutingDnsTls),
                    (DnsType::Https, Key::RoutingDnsHttps),
                ],
            );
            if matches!(draft.dns_type, DnsType::Tcp | DnsType::Tls | DnsType::Https) {
                divider(ui);
                text_row(ui, locale, Key::RoutingDnsEndpoint, &mut draft.dns_endpoint);
            }
            if matches!(draft.dns_type, DnsType::Tls | DnsType::Https) {
                divider(ui);
                text_row(
                    ui,
                    locale,
                    Key::RoutingDnsDomain,
                    &mut draft.dns_server_name,
                );
            }
            if draft.dns_type == DnsType::Https {
                divider(ui);
                text_row(ui, locale, Key::RoutingDnsPath, &mut draft.dns_path);
            }
        },
    );
}

fn rule_editor(ui: &mut egui::Ui, locale: Locale, rules: &mut Vec<RouteRule>) {
    let tr = |key| localization::text(locale, key);
    group(ui, Some(tr(Key::RoutingRules)), |ui| {
        ui.weak(tr(Key::RoutingFirstMatch));
        let mut edit = None;
        let count = rules.len();
        for (index, rule) in rules.iter_mut().enumerate() {
            divider(ui);
            let action = ui
                .push_id(index, |ui| rule_row(ui, locale, rule, index, count))
                .inner;
            edit = action.or(edit);
        }
        apply_rule_edit(rules, edit);
        if ui.button(tr(Key::RoutingAddRule)).clicked() {
            rules.push(RouteRule {
                target: RouteTarget::Domain(String::new()),
                action: RouteAction::Proxy,
            });
        }
    });
}

enum RuleEdit {
    Swap(usize, usize),
    Remove(usize),
}

fn apply_rule_edit(rules: &mut Vec<RouteRule>, action: Option<RuleEdit>) {
    match action {
        Some(RuleEdit::Swap(left, right)) => rules.swap(left, right),
        Some(RuleEdit::Remove(index)) => {
            rules.remove(index);
        }
        None => {}
    }
}

fn rule_row(
    ui: &mut egui::Ui,
    locale: Locale,
    rule: &mut RouteRule,
    index: usize,
    count: usize,
) -> Option<RuleEdit> {
    let tr = |key| localization::text(locale, key);
    choice_row(
        ui,
        locale,
        Key::RoutingRuleAction,
        &mut rule.action,
        &[
            (RouteAction::Proxy, Key::RoutingProxy),
            (RouteAction::Direct, Key::RoutingDirect),
            (RouteAction::Block, Key::RoutingBlock),
        ],
    );
    target_editor(ui, locale, &mut rule.target);
    ui.horizontal_wrapped(|ui| {
        if ui
            .add_enabled(index > 0, egui::Button::new(tr(Key::RoutingMoveUp)))
            .clicked()
        {
            return Some(RuleEdit::Swap(index, index - 1));
        }
        if ui
            .add_enabled(
                index + 1 < count,
                egui::Button::new(tr(Key::RoutingMoveDown)),
            )
            .clicked()
        {
            return Some(RuleEdit::Swap(index, index + 1));
        }
        ui.button(tr(Key::Remove))
            .clicked()
            .then_some(RuleEdit::Remove(index))
    })
    .inner
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TargetKind {
    Domain,
    Suffix,
    Cidr,
}

fn target_editor(ui: &mut egui::Ui, locale: Locale, target: &mut RouteTarget) {
    let (original, value) = match target {
        RouteTarget::Domain(value) => (TargetKind::Domain, value),
        RouteTarget::DomainSuffix(value) => (TargetKind::Suffix, value),
        RouteTarget::Cidr(value) => (TargetKind::Cidr, value),
    };
    let mut kind = original;
    choice_row(
        ui,
        locale,
        Key::RoutingRuleTarget,
        &mut kind,
        &[
            (TargetKind::Domain, Key::RoutingExactDomain),
            (TargetKind::Suffix, Key::RoutingDomainSuffix),
            (TargetKind::Cidr, Key::RoutingCidr),
        ],
    );
    text_row(ui, locale, Key::RoutingRuleValue, value);
    if kind != original {
        let value = std::mem::take(value);
        *target = match kind {
            TargetKind::Domain => RouteTarget::Domain(value),
            TargetKind::Suffix => RouteTarget::DomainSuffix(value),
            TargetKind::Cidr => RouteTarget::Cidr(value),
        };
    }
}

fn unavailable_menu_item(ui: &mut egui::Ui, locale: Locale, key: Key) {
    ui.add_enabled(false, egui::Button::new(localization::text(locale, key)))
        .on_disabled_hover_text(localization::text(locale, Key::SettingUnavailable));
}

struct DialogButtons {
    confirm: bool,
    cancel: bool,
}

fn routing_dialog_width(ui: &egui::Ui) -> f32 {
    ROUTING_DIALOG_WIDTH.min((ui.ctx().content_rect().width() - 24.0).max(0.0))
}

fn dialog_buttons(ui: &mut egui::Ui, locale: Locale, confirm: Key, enabled: bool) -> DialogButtons {
    ui.with_layout(
        egui::Layout::right_to_left(egui::Align::Center).with_main_wrap(true),
        |ui| {
            let confirm = ui
                .add_enabled(
                    enabled,
                    egui::Button::new(localization::text(locale, confirm)),
                )
                .clicked();
            let cancel = ui.button(localization::text(locale, Key::Cancel)).clicked();
            DialogButtons { confirm, cancel }
        },
    )
    .inner
}
