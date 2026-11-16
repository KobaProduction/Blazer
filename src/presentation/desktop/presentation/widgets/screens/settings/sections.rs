use std::net::{IpAddr, SocketAddr};

use eframe::egui;
use vpn_presentation::localization::{self, Key, Locale};
use vpn_presentation::model::Intent;
use vpn_presentation::session::ConnectionMode;

use super::{
    ENTRY_WIDTH, GROUP_GAP, ViewFrame, choice, choice_row, divider, group, link_row, setting_row,
    toggle_row,
};
use crate::presentation::settings_state::{
    AdvancedSettings, PerAppMode, PingDisplay, PingMethod, ServerSort, SubscriptionSettings,
};
use crate::presentation::view_model::SettingsPage;
use crate::presentation::widgets::controls::roomy_input;

impl ViewFrame<'_> {
    pub(super) fn interface_rows(&mut self, ui: &mut egui::Ui) {
        let locale = self.view.locale;
        let tr = |key| localization::text(locale, key);
        let system_label = system_scale_label(ui.ctx(), locale);
        let interface = &mut self.view.desktop_settings.interface;
        setting_row(ui, tr(Key::DesktopScale), |ui| {
            choice(
                ui,
                Key::DesktopScale.id(),
                &mut interface.scale_percent,
                &[
                    (None, &system_label),
                    (Some(75), "75%"),
                    (Some(90), "90%"),
                    (Some(100), "100%"),
                    (Some(110), "110%"),
                    (Some(125), "125%"),
                    (Some(150), "150%"),
                    (Some(175), "175%"),
                    (Some(200), "200%"),
                ],
            );
        });
        divider(ui);
        setting_row(ui, tr(Key::GraphicsBackend), |ui| {
            ui.label("OpenGL")
                .on_hover_text(tr(Key::GraphicsBackendHelp));
        });
    }

    pub(super) fn advanced_settings(&mut self, ui: &mut egui::Ui) {
        let locale = self.view.locale;
        group(ui, None, |ui| {
            let advanced = &mut self.view.desktop_settings.advanced;
            for (label, value) in [
                (Key::LocalDns, &mut advanced.local_dns),
                (Key::JsonDns, &mut advanced.json_dns),
                (Key::ResolveServer, &mut advanced.resolve_server),
                (Key::Sniffing, &mut advanced.sniffing),
                (Key::Autostart, &mut advanced.autostart),
            ] {
                toggle_row(ui, locale, label, value);
                divider(ui);
            }
            self.mode_toggle(ui, Key::ModeSystemProxy, ConnectionMode::SystemProxy);
            divider(ui);
            toggle_row(
                ui,
                locale,
                Key::BypassLocalProxy,
                &mut self.view.desktop_settings.advanced.bypass_local_proxy,
            );
            divider(ui);
            self.mode_toggle(ui, Key::EnableTun, ConnectionMode::Tun);
            divider(ui);
            self.page_link(ui, Key::ExcludedRoutes, SettingsPage::ExcludedRoutes);
        });
        ui.weak(self.tr(Key::SettingsPreviewOnly));
    }

    pub(super) fn excluded_routes(&mut self, ui: &mut egui::Ui) {
        let locale = self.view.locale;
        let tr = |key| localization::text(locale, key);
        let advanced = &mut self.view.desktop_settings.advanced;
        group(ui, Some(tr(Key::CidrList)), |ui| {
            removable_rows(ui, locale, &mut advanced.excluded_routes);
            cidr_row(ui, locale, advanced);
        });
        ui.weak(tr(Key::CidrHelp));
        ui.add_space(GROUP_GAP);
        ui.weak(tr(Key::SettingsPreviewOnly));
    }

    pub(super) fn per_app_proxy(&mut self, ui: &mut egui::Ui) {
        if !self.tun_selected() {
            ui.weak(self.tr(Key::PerAppTunRequired));
            return;
        }
        let locale = self.view.locale;
        let tr = |key| localization::text(locale, key);
        let tunnel = &mut self.view.desktop_settings.tunnel;
        group(ui, None, |ui| {
            choice_row(
                ui,
                locale,
                Key::Mode,
                &mut tunnel.per_app_mode,
                &[
                    (PerAppMode::System, Key::PerAppSystem),
                    (PerAppMode::Direct, Key::PerAppDirect),
                    (PerAppMode::ProxyOnly, Key::PerAppOnly),
                ],
            );
        });
        group(ui, Some(tr(Key::PerAppList)), |ui| {
            removable_rows(ui, locale, &mut tunnel.excluded_apps);
            setting_row(ui, tr(Key::PerAppExecutable), |ui| {
                let enabled = !tunnel.app_input.trim().is_empty();
                let add = ui
                    .add_enabled(enabled, egui::Button::new(tr(Key::Add)))
                    .clicked();
                let input = ui.add(
                    roomy_input(&mut tunnel.app_input, ENTRY_WIDTH.min(ui.available_width()))
                        .id_salt(Key::PerAppExecutable.id()),
                );
                if add || (input.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))) {
                    let path = std::mem::take(&mut tunnel.app_input);
                    let path = path.trim();
                    if !path.is_empty() && !tunnel.excluded_apps.iter().any(|app| app == path) {
                        tunnel.excluded_apps.push(path.to_owned());
                    }
                }
            });
        });
        ui.weak(tr(Key::SettingsPreviewOnly));
    }

    pub(super) fn subscription_settings(&mut self, ui: &mut egui::Ui) {
        let locale = self.view.locale;
        ui.label(self.tr(Key::SubscriptionScheduleDraft));
        ui.add_space(4.0);
        let settings = &mut self.view.desktop_settings.subscriptions;
        subscription_update_rows(ui, locale, settings);
        subscription_behavior_rows(ui, locale, settings);
        ui.weak(self.tr(Key::SettingsPreviewOnly));
        ui.add_space(GROUP_GAP);
        self.live_subscription_settings(ui);
        group(ui, Some(self.tr(Key::ServerSort)), |ui| {
            for (sort, key) in [
                (None, Key::Automatic),
                (Some(ServerSort::Original), Key::SortOriginal),
                (Some(ServerSort::Ping), Key::SortPing),
                (Some(ServerSort::Alphabetical), Key::SortAlphabetical),
            ] {
                ui.radio_value(
                    &mut self
                        .view
                        .desktop_settings
                        .subscriptions
                        .server_sort_override,
                    sort,
                    localization::text(locale, key),
                );
            }
        });
        self.subscription_links(ui);
    }

    fn live_subscription_settings(&mut self, ui: &mut egui::Ui) {
        let mut send_hwid = self.snapshot.send_subscription_hwid;
        let locale = self.view.locale;
        group(ui, None, |ui| {
            toggle_row(
                ui,
                locale,
                Key::UpdateNotification,
                &mut self.view.desktop_settings.subscriptions.notify_on_update,
            );
            divider(ui);
            ui.add_enabled_ui(!self.snapshot.busy, |ui| {
                toggle_row(ui, locale, Key::SendHwid, &mut send_hwid);
            });
        });
        if send_hwid != self.snapshot.send_subscription_hwid {
            self.emit(Intent::SetSubscriptionHwid(send_hwid));
        }
    }

    fn subscription_links(&mut self, ui: &mut egui::Ui) {
        if self.snapshot.subscriptions.is_empty() {
            return;
        }
        group(ui, Some(self.tr(Key::ManageSubscriptions)), |ui| {
            for (index, source) in self.snapshot.subscriptions.iter().enumerate() {
                if index != 0 {
                    divider(ui);
                }
                ui.push_id(&source.subscription_uuid, |ui| {
                    if link_row(ui, &source.title) {
                        self.view.open_provider_settings(&source.subscription_uuid);
                    }
                });
            }
        });
    }

    pub(super) fn ping_settings(&mut self, ui: &mut egui::Ui) {
        let locale = self.view.locale;
        let ping = &mut self.view.desktop_settings.ping;
        group(ui, None, |ui| {
            choice_row(
                ui,
                locale,
                Key::PingMethod,
                &mut ping.method,
                &[(PingMethod::Tcp, Key::PingTcp)],
            );
            divider(ui);
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
        ui.weak(localization::text(locale, Key::PingMethodHelp));
    }

    pub(super) fn inbounds(&mut self, ui: &mut egui::Ui) {
        let locale = self.view.locale;
        let tr = |key| localization::text(locale, key);
        let live = live_port(self.snapshot.local_proxy.as_deref());
        let saved = self.snapshot.local_proxy_port;
        let enabled = self.snapshot.mode_capabilities.local_proxy && !self.snapshot.busy;
        let draft = &mut self.view.desktop_settings.local_proxy_port_draft;
        let mut update = None;
        group(ui, Some(tr(Key::InboundListener)), |ui| {
            update = inbound_port(ui, locale, saved, enabled, draft);
            if let Some(port) = live {
                divider(ui);
                setting_row(ui, tr(Key::InboundLivePort), |ui| {
                    ui.label(port.to_string());
                });
            }
        });
        if let Some(port) = update {
            self.emit(Intent::SetLocalProxyPort(port));
        }
        ui.weak(tr(Key::InboundPortHelp));
        if self.view.preview {
            for (index, title) in ["SOCKS5", "HTTP CONNECT"].into_iter().enumerate() {
                ui.push_id(title, |ui| {
                    group(ui, Some(title), |ui| {
                        choice_row(
                            ui,
                            locale,
                            Key::InboundAuthMode,
                            &mut self.view.desktop_settings.inbound_auth[index],
                            &[(false, Key::InboundNoAuth), (true, Key::InboundPassword)],
                        );
                    });
                });
            }
            ui.weak(tr(Key::SettingsPreviewOnly));
        } else {
            ui.weak(tr(Key::InboundAuthUnavailable));
        }
    }
}

fn system_scale_label(context: &egui::Context, locale: Locale) -> String {
    let native = context.native_pixels_per_point().unwrap_or(1.0);
    let scale = format!("{:.0}%", native * 100.0);
    localization::format(locale, Key::DesktopScaleSystem, &[("scale", &scale)]).unwrap_or(scale)
}

fn removable_rows(ui: &mut egui::Ui, locale: Locale, values: &mut Vec<String>) {
    let mut remove = None;
    for (index, label) in values.iter().enumerate() {
        setting_row(ui, label, |ui| {
            if ui.button(localization::text(locale, Key::Remove)).clicked() {
                remove = Some(index);
            }
        });
        divider(ui);
    }
    if let Some(index) = remove {
        values.remove(index);
    }
}

fn cidr_row(ui: &mut egui::Ui, locale: Locale, advanced: &mut AdvancedSettings) {
    let tr = |key| localization::text(locale, key);
    let input_id = ui.make_persistent_id("cidr-input");
    let Some(input) = &mut advanced.cidr_input else {
        if link_row(ui, tr(Key::AddCidr)) {
            advanced.cidr_input = Some(String::new());
            ui.memory_mut(|memory| memory.request_focus(input_id));
        }
        return;
    };
    ui.add_space(4.0);
    let keys = ui.input(|state| {
        (
            state.key_pressed(egui::Key::Enter),
            state.key_pressed(egui::Key::Escape),
        )
    });
    let response = ui.add(
        roomy_input(input, ui.available_width())
            .id(input_id)
            .hint_text(tr(Key::CidrInputHint)),
    );
    let focused = response.has_focus() || response.lost_focus();
    let mut cancel = focused && keys.1;
    if response.changed() {
        advanced.invalid_cidr = false;
    }
    let add = focused && keys.0;
    if add {
        let value = input.trim();
        if valid_cidr(value) {
            if !advanced.excluded_routes.iter().any(|route| route == value) {
                advanced.excluded_routes.push(value.to_owned());
            }
            cancel = true;
        } else {
            advanced.invalid_cidr = true;
            ui.memory_mut(|memory| memory.request_focus(input_id));
        }
    }
    if cancel {
        advanced.cidr_input = None;
        advanced.invalid_cidr = false;
    }
    if advanced.invalid_cidr {
        ui.colored_label(ui.visuals().error_fg_color, tr(Key::InvalidInput));
    } else if !cancel {
        ui.weak(tr(Key::CidrKeysHelp));
    }
}

fn valid_cidr(value: &str) -> bool {
    let Some((address, prefix)) = value.split_once('/') else {
        return false;
    };
    match (address.parse::<IpAddr>(), prefix.parse::<u8>()) {
        (Ok(IpAddr::V4(_)), Ok(prefix)) => prefix <= 32,
        (Ok(IpAddr::V6(_)), Ok(prefix)) => prefix <= 128,
        _ => false,
    }
}

fn subscription_update_rows(
    ui: &mut egui::Ui,
    locale: Locale,
    settings: &mut SubscriptionSettings,
) {
    group(ui, None, |ui| {
        toggle_row(ui, locale, Key::AutoUpdate, &mut settings.auto_update);
        divider(ui);
        number_row(
            ui,
            locale,
            Key::UpdateInterval,
            &mut settings.interval_hours,
            1..=168,
            Key::HoursValue,
        );
        divider(ui);
        number_row(
            ui,
            locale,
            Key::RequestTimeout,
            &mut settings.timeout_seconds,
            1..=300,
            Key::SecondsValue,
        );
        for (key, value) in [
            (Key::UpdateOnLaunch, &mut settings.update_on_open),
            (Key::PingOnLaunch, &mut settings.ping_on_open),
            (Key::ConnectOnLaunch, &mut settings.connect_on_open),
        ] {
            divider(ui);
            toggle_row(ui, locale, key, value);
        }
    });
}

fn subscription_behavior_rows(
    ui: &mut egui::Ui,
    locale: Locale,
    settings: &mut SubscriptionSettings,
) {
    group(ui, None, |ui| {
        for (index, (key, value)) in [
            (Key::PreventDuplicates, &mut settings.prevent_duplicates),
            (Key::CollapseSubscriptions, &mut settings.collapse),
            (Key::DisableSubscriptionFilter, &mut settings.disable_filter),
        ]
        .into_iter()
        .enumerate()
        {
            if index != 0 {
                divider(ui);
            }
            toggle_row(ui, locale, key, value);
        }
    });
}

fn number_row(
    ui: &mut egui::Ui,
    locale: Locale,
    key: Key,
    value: &mut u16,
    range: std::ops::RangeInclusive<u16>,
    format_key: Key,
) {
    let suffix = localization::text(locale, format_key).trim_start_matches("{value}");
    setting_row(ui, localization::text(locale, key), |ui| {
        ui.add(egui::DragValue::new(value).range(range).suffix(suffix));
    });
}

fn live_port(endpoint: Option<&str>) -> Option<u16> {
    Some(endpoint?.parse::<SocketAddr>().ok()?.port())
}

fn inbound_port(
    ui: &mut egui::Ui,
    locale: Locale,
    saved: u16,
    enabled: bool,
    draft: &mut Option<String>,
) -> Option<u16> {
    let tr = |key| localization::text(locale, key);
    let mut input = draft.clone().unwrap_or_else(|| saved.to_string());
    let mut submit = false;
    let mut focused = false;
    setting_row(ui, tr(Key::InboundPort), |ui| {
        let response = ui
            .add_enabled(
                enabled,
                roomy_input(&mut input, 96.0)
                    .id_salt(Key::InboundPort.id())
                    .hint_text(tr(Key::Automatic))
                    .char_limit(5),
            )
            .on_disabled_hover_text(tr(Key::SettingUnavailable));
        if response.changed() {
            *draft = Some(input.clone());
        }
        focused = response.has_focus();
        submit = response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
    });
    let port = input.parse::<u16>().ok();
    if port == Some(saved) && !focused {
        *draft = None;
    }
    if port.is_none() {
        ui.colored_label(ui.visuals().error_fg_color, tr(Key::InboundPortInvalid));
    }
    let can_save = enabled && port.is_some_and(|value| value != saved);
    let clicked = ui
        .add_enabled(can_save, egui::Button::new(tr(Key::Save)))
        .clicked();
    port.filter(|_| can_save && (submit || clicked))
}
