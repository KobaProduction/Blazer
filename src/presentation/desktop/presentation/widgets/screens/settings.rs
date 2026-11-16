mod routing;
mod sections;
mod telemetry;
mod url_schemes;

use eframe::egui;
use vpn_presentation::localization::{self, Key, Locale};
use vpn_presentation::model::{self, Intent};
use vpn_presentation::session::{ConnectionMode, Language};

use super::ViewFrame;
use crate::presentation::settings_state::{DesktopSettings, DesktopTheme, PreferredIp};
use crate::presentation::view_model::{Screen, SettingsPage};
use crate::presentation::widgets::controls::{roomy_input, select_value, selector};

pub(super) const ROW_HEIGHT: f32 = 44.0;
const GROUP_GAP: f32 = 14.0;
const CONTROL_WIDTH: f32 = 205.0;
const ENTRY_WIDTH: f32 = 220.0;
const GROUP_MARGIN: i8 = 12;
const COMPACT_ROW_WIDTH: f32 = 335.0;
const ROW_INSET: i8 = 10;
const ROW_CONTENT_GAP: f32 = 12.0;

impl ViewFrame<'_> {
    pub(super) fn settings(&mut self, ui: &mut egui::Ui, page: SettingsPage) {
        match page {
            SettingsPage::Index => self.settings_index(ui),
            SettingsPage::Advanced => self.advanced_settings(ui),
            SettingsPage::ExcludedRoutes => self.excluded_routes(ui),
            SettingsPage::Routing => self.routing_settings(ui),
            SettingsPage::RoutingProfile => self.routing_profile(ui),
            SettingsPage::SubscriptionRouting => self.subscription_routing(ui),
            SettingsPage::Subscriptions => self.subscription_settings(ui),
            SettingsPage::Subscription => self.provider_settings(ui),
            SettingsPage::Ping => self.ping_settings(ui),
            SettingsPage::Inbounds => self.inbounds(ui),
            SettingsPage::PerAppProxy => self.per_app_proxy(ui),
            SettingsPage::UrlSchemes => self.url_schemes(ui),
            SettingsPage::Logs => self.logs(ui),
            SettingsPage::Telemetry => self.telemetry_settings(ui),
            SettingsPage::About => self.about(ui),
        }
        if matches!(
            page,
            SettingsPage::Routing
                | SettingsPage::SubscriptionRouting
                | SettingsPage::RoutingProfile
        ) {
            self.routing_dialog(ui);
        }
    }

    fn settings_index(&mut self, ui: &mut egui::Ui) {
        let locale = self.view.locale;
        group(ui, Some(self.tr(Key::InterfaceSettings)), |ui| {
            setting_row(ui, self.tr(Key::Language), |ui| self.language_picker(ui));
            divider(ui);
            choice_row(
                ui,
                locale,
                Key::Theme,
                &mut self.view.desktop_settings.interface.theme,
                &[
                    (DesktopTheme::System, Key::ThemeSystem),
                    (DesktopTheme::Dark, Key::ThemeDark),
                    (DesktopTheme::Light, Key::ThemeLight),
                ],
            );
            divider(ui);
            self.interface_rows(ui);
        });
        self.tunnel_settings(ui);
        self.advanced_links(ui);
        self.other_settings(ui);
    }

    fn tunnel_settings(&mut self, ui: &mut egui::Ui) {
        let locale = self.view.locale;
        group(ui, Some(self.tr(Key::TunnelSettings)), |ui| {
            self.page_link(ui, Key::RoutingRules, SettingsPage::Routing);
            divider(ui);
            if self.view.preview && self.tun_selected() {
                self.page_link(ui, Key::PerAppProxy, SettingsPage::PerAppProxy);
                divider(ui);
            }
            let settings = &mut self.view.desktop_settings.tunnel;
            toggle_row(ui, locale, Key::Fragmentation, &mut settings.fragmentation);
            divider(ui);
            toggle_row(ui, locale, Key::Multiplex, &mut settings.multiplex);
            divider(ui);
            setting_row(ui, self.tr(Key::PreferredIp), |ui| {
                choice(
                    ui,
                    Key::PreferredIp.id(),
                    &mut self.view.desktop_settings.tunnel.preferred_ip,
                    &[
                        (
                            PreferredIp::Automatic,
                            localization::text(locale, Key::Automatic),
                        ),
                        (PreferredIp::V4, "IPv4"),
                        (PreferredIp::V6, "IPv6"),
                    ],
                );
            });
            divider(ui);
            self.page_link(ui, Key::Inbounds, SettingsPage::Inbounds);
        });
        ui.weak(self.tr(Key::SettingsPreviewOnly));
        ui.add_space(GROUP_GAP);
    }

    fn advanced_links(&mut self, ui: &mut egui::Ui) {
        let locale = self.view.locale;
        group(ui, Some(self.tr(Key::AdvancedSettings)), |ui| {
            self.page_link(ui, Key::AdvancedSettings, SettingsPage::Advanced);
            divider(ui);
            self.page_link(ui, Key::Subscriptions, SettingsPage::Subscriptions);
            divider(ui);
            self.page_link(ui, Key::Ping, SettingsPage::Ping);
            divider(ui);
            toggle_row(
                ui,
                locale,
                Key::AllowLan,
                &mut self.view.desktop_settings.tunnel.allow_lan,
            );
        });
    }

    fn other_settings(&mut self, ui: &mut egui::Ui) {
        group(ui, Some(self.tr(Key::OtherSettings)), |ui| {
            self.page_link(ui, Key::Logs, SettingsPage::Logs);
            divider(ui);
            self.page_link(ui, Key::OpenTelemetry, SettingsPage::Telemetry);
            divider(ui);
            if link_row(ui, self.tr(Key::ResetSettings)) {
                self.view.desktop_settings.confirm_reset = true;
            }
        });
        group(ui, Some(self.tr(Key::About)), |ui| {
            if link_row(ui, self.tr(Key::Faq)) {
                let url = match self.view.locale {
                    Locale::Russian => "https://www.happ.su/main/ru/faq",
                    Locale::English => "https://www.happ.su/main/faq",
                };
                ui.ctx().open_url(egui::OpenUrl::new_tab(url));
            }
            divider(ui);
            self.page_link(ui, Key::UrlSchemes, SettingsPage::UrlSchemes);
            divider(ui);
            self.page_link(ui, Key::About, SettingsPage::About);
        });
        self.reset_settings_dialog(ui);
    }

    fn reset_settings_dialog(&mut self, ui: &mut egui::Ui) {
        if !self.view.desktop_settings.confirm_reset {
            return;
        }
        let mut reset = false;
        let mut cancel = false;
        let modal =
            egui::Modal::new(ui.make_persistent_id("reset-settings")).show(ui.ctx(), |ui| {
                ui.heading(self.tr(Key::ResetSettings));
                ui.label(self.tr(Key::ResetSettingsConfirm));
                ui.horizontal(|ui| {
                    cancel = ui.button(self.tr(Key::Cancel)).clicked();
                    reset = ui.button(self.tr(Key::ResetSettings)).clicked();
                });
            });
        if reset {
            self.view.desktop_settings = DesktopSettings::default();
            self.view.subscription_routing_uuid = None;
        } else if cancel || modal.should_close() {
            self.view.desktop_settings.confirm_reset = false;
        }
    }

    fn language_picker(&mut self, ui: &mut egui::Ui) {
        let mut language = self.snapshot.language;
        choice(
            ui,
            Key::Language.id(),
            &mut language,
            &[
                (Language::Russian, self.tr(Key::Russian)),
                (Language::English, self.tr(Key::English)),
            ],
        );
        if language != self.snapshot.language {
            self.emit(Intent::ChangeLanguage(language));
        }
    }

    fn page_link(&mut self, ui: &mut egui::Ui, key: Key, page: SettingsPage) {
        if link_row(ui, self.tr(key)) {
            self.view.open(Screen::Settings(page));
        }
    }

    fn tun_selected(&self) -> bool {
        self.snapshot.selected_mode == ConnectionMode::Tun && self.snapshot.mode_capabilities.tun
    }

    fn mode_toggle(&mut self, ui: &mut egui::Ui, key: Key, mode: ConnectionMode) {
        let is_selected = self.snapshot.selected_mode == mode;
        let available = model::mode_available(mode, self.snapshot.mode_capabilities);
        let idle = !(self.snapshot.busy || self.saving_mode || self.snapshot.cleanup_required)
            && self.snapshot.active_mode.is_none();
        let other_mode =
            model::mode_available(ConnectionMode::LocalProxy, self.snapshot.mode_capabilities);
        let enabled = available && idle && (!is_selected || other_mode);
        let reason = if !available || (is_selected && !other_mode) {
            Key::ModeUnavailable
        } else if self.snapshot.busy || self.saving_mode {
            Key::Working
        } else {
            Key::ModeFinishFirst
        };
        let mut next = is_selected;
        let locale = self.view.locale;
        setting_row(ui, self.tr(key), |ui| {
            ui.add_enabled_ui(enabled, |ui| toggle(ui, locale, key, &mut next))
                .inner
                .on_disabled_hover_text(self.tr(reason));
        });
        if next != is_selected {
            self.emit(Intent::ChangeMode(if next {
                mode
            } else {
                ConnectionMode::LocalProxy
            }));
        }
    }
}

pub(super) fn group(ui: &mut egui::Ui, title: Option<&str>, rows: impl FnOnce(&mut egui::Ui)) {
    if let Some(title) = title {
        ui.strong(title);
        ui.add_space(4.0);
    }
    egui::Frame::new()
        .fill(ui.visuals().faint_bg_color)
        .corner_radius(5)
        .inner_margin(egui::Margin::symmetric(GROUP_MARGIN, 3))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 0.0;
            rows(ui);
        });
    ui.add_space(GROUP_GAP);
}

pub(super) fn divider(ui: &mut egui::Ui) {
    ui.add(egui::Separator::default().spacing(0.0));
}

fn row_note(ui: &mut egui::Ui, text: &str) {
    egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(ROW_INSET, 4))
        .show(ui, |ui| {
            ui.add(egui::Label::new(egui::RichText::new(text).weak()).wrap());
        });
}

fn link_row(ui: &mut egui::Ui, label: &str) -> bool {
    const HIGHLIGHT_INSET: i8 = 2;
    egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(0, HIGHLIGHT_INSET))
        .show(ui, |ui| {
            ui.spacing_mut().button_padding = egui::vec2(f32::from(ROW_INSET), 4.0);
            let visuals = ui.visuals_mut();
            for state in [&mut visuals.widgets.hovered, &mut visuals.widgets.active] {
                state.expansion = 0.0;
            }
            let title = egui::RichText::new(label).color(ui.visuals().strong_text_color());
            let chevron = egui::Atom::paint(egui::vec2(8.0, 12.0), |ui, args| {
                let center = args.rect.center();
                ui.painter().add(egui::Shape::line(
                    vec![
                        center + egui::vec2(-2.0, -4.0),
                        center + egui::vec2(2.0, 0.0),
                        center + egui::vec2(-2.0, 4.0),
                    ],
                    egui::Stroke::new(1.5, args.fallback_text_color),
                ));
            });
            ui.add_sized(
                [
                    ui.available_width(),
                    ROW_HEIGHT - f32::from(2 * HIGHLIGHT_INSET),
                ],
                egui::Button::new(title)
                    .right_text(chevron)
                    .gap(ROW_CONTENT_GAP)
                    .corner_radius(5)
                    .frame_when_inactive(false)
                    .truncate(),
            )
            .clicked()
        })
        .inner
}

pub(super) fn setting_row(ui: &mut egui::Ui, label: &str, value: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(ROW_INSET, 0))
        .show(ui, |ui| {
            let width = ui.available_width();
            let label = egui::RichText::new(label).color(ui.visuals().strong_text_color());
            let value_layout = egui::Layout::right_to_left(egui::Align::Center);
            if width < COMPACT_ROW_WIDTH {
                ui.set_min_width(width);
                ui.add_space(4.0);
                ui.add(egui::Label::new(label).wrap());
                ui.add_space(4.0);
                ui.with_layout(value_layout.with_main_wrap(true), value);
                ui.add_space(4.0);
                return;
            }
            let label_width = (width - ENTRY_WIDTH - ROW_CONTENT_GAP).min(width * 0.63);
            ui.allocate_ui_with_layout(
                egui::vec2(width, ROW_HEIGHT),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    ui.spacing_mut().item_spacing.x = ROW_CONTENT_GAP;
                    ui.allocate_ui_with_layout(
                        egui::vec2(label_width, ROW_HEIGHT),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| ui.add(egui::Label::new(label).wrap()),
                    );
                    ui.with_layout(value_layout, value);
                },
            );
        });
}

fn toggle_row(ui: &mut egui::Ui, locale: Locale, key: Key, value: &mut bool) {
    setting_row(ui, localization::text(locale, key), |ui| {
        toggle(ui, locale, key, value);
    });
}

pub(in crate::presentation::widgets) fn toggle(
    ui: &mut egui::Ui,
    locale: Locale,
    key: Key,
    value: &mut bool,
) -> egui::Response {
    let (rect, mut response) = ui.allocate_exact_size(egui::vec2(34.0, 19.0), egui::Sense::click());
    if response.clicked() {
        *value = !*value;
        response.mark_changed();
    }
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::Role::Switch,
            ui.is_enabled(),
            *value,
            localization::text(locale, key),
        )
    });
    let visuals = ui.visuals();
    let (fill, thumb) = if *value {
        (visuals.hyperlink_color, visuals.extreme_bg_color)
    } else {
        (visuals.widgets.inactive.bg_fill, visuals.text_color())
    };
    let thumb_x = if *value {
        rect.right() - 9.5
    } else {
        rect.left() + 9.5
    };
    let stroke = ui.style().interact(&response).bg_stroke;
    ui.painter()
        .rect(rect, 10, fill, stroke, egui::StrokeKind::Inside);
    ui.painter()
        .circle_filled(egui::pos2(thumb_x, rect.center().y), 6.5, thumb);
    let state = if *value {
        Key::SettingEnabled
    } else {
        Key::SettingDisabled
    };
    ui.label(localization::text(locale, state));
    response
}

fn choice<T: Copy + PartialEq>(ui: &mut egui::Ui, id: &str, value: &mut T, options: &[(T, &str)]) {
    let label = options
        .iter()
        .find(|(option, _)| *option == *value)
        .map_or("", |(_, label)| *label);
    selector(ui, id, CONTROL_WIDTH, label, |ui| {
        for &(option, label) in options {
            select_value(ui, value, option, label);
        }
    });
}

fn choice_row<T: Copy + PartialEq>(
    ui: &mut egui::Ui,
    locale: Locale,
    key: Key,
    value: &mut T,
    options: &[(T, Key)],
) {
    let label = options
        .iter()
        .find(|(option, _)| *option == *value)
        .map_or("", |(_, key)| localization::text(locale, *key));
    setting_row(ui, localization::text(locale, key), |ui| {
        selector(ui, key.id(), CONTROL_WIDTH, label, |ui| {
            for &(option, key) in options {
                select_value(ui, value, option, localization::text(locale, key));
            }
        });
    });
}

fn text_row(ui: &mut egui::Ui, locale: Locale, key: Key, value: &mut String) {
    let label = localization::text(locale, key);
    setting_row(ui, label, |ui| {
        ui.add(
            roomy_input(value, ENTRY_WIDTH.min(ui.available_width()))
                .id_salt(key.id())
                .hint_text(label),
        );
    });
}
