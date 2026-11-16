use eframe::egui;
use vpn_presentation::session::{ProfileLatency, ProfileSummary, UnavailableProfile};

use super::ViewFrame;
use crate::presentation::settings_state::PingDisplay;
use crate::presentation::theme;
use crate::presentation::view_model::CatalogOperation;
use crate::presentation::widgets::connection_controls::CONNECTION_CONTROL_WIDTH;
use crate::presentation::widgets::controls::SELECTOR_HEIGHT;
use crate::presentation::widgets::flags::{paint_flag, server_name};
use crate::presentation::widgets::icons::paint_failure_badge;
use vpn_presentation::localization::Key;
use vpn_presentation::model::{self, Intent};

impl ViewFrame<'_> {
    pub(super) fn profile_row(
        &mut self,
        ui: &mut egui::Ui,
        profile: &ProfileSummary,
        ping: PingDisplay,
    ) {
        let selected = self.snapshot.selected_profile_uuid.as_ref() == Some(&profile.profile_uuid);
        ui.push_id(("server", &profile.profile_uuid), |ui| {
            let (rect, _) = ui
                .allocate_exact_size(egui::vec2(ui.available_width(), 52.0), egui::Sense::hover());
            let compact = rect.width() < 225.0;
            let action_width = if compact { 28.0 } else { 38.0 };
            let split = rect.right() - action_width;
            let choice = egui::Rect::from_min_max(rect.min, egui::pos2(split, rect.bottom()));
            let response = self.profile_choice(ui, choice, profile, selected);
            paint_row(ui, rect, selected, &response);
            server_identity(
                ui,
                choice,
                &profile.name,
                &profile.protocol,
                compact,
                if compact { 51.0 } else { 65.0 },
            );
            self.profile_ping(ui, choice, profile, compact, ping);
            self.profile_editor_button(
                ui,
                egui::Rect::from_min_max(egui::pos2(split, rect.top()), rect.max),
                profile,
            );
        });
    }

    pub(super) fn unavailable_profile_row(
        &mut self,
        ui: &mut egui::Ui,
        profile: &UnavailableProfile,
        display: PingDisplay,
        subscription_uuid: Option<&str>,
    ) {
        let (rect, _) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 52.0), egui::Sense::hover());
        let locale = self.view.locale;
        let name = model::unavailable_profile_name(locale, profile);
        let reason = model::unavailable_profile_reason(locale, profile.reason);
        let subtitle = profile
            .protocol
            .as_ref()
            .map(|protocol| format!("{reason} · {protocol}"));
        let control_width = if profile.probe_id.is_some() {
            (rect.width() * 0.4).min(84.0)
        } else {
            0.0
        };
        server_identity(
            ui,
            rect,
            name,
            subtitle.as_deref().unwrap_or(reason),
            rect.width() < 225.0,
            control_width + 6.0,
        );
        if let Some(id) = profile.probe_id.as_deref() {
            let control = egui::Rect::from_center_size(
                egui::pos2(rect.right() - control_width * 0.5, rect.center().y),
                egui::vec2(control_width, SELECTOR_HEIGHT),
            );
            self.unavailable_ping_button(ui, control, id, display, subscription_uuid);
        }
    }

    fn unavailable_ping_button(
        &mut self,
        ui: &mut egui::Ui,
        rect: egui::Rect,
        id: &str,
        display: PingDisplay,
        subscription_uuid: Option<&str>,
    ) {
        if self.probe_pending(id, subscription_uuid) {
            egui::Spinner::new().paint_at(
                ui,
                egui::Rect::from_center_size(rect.center(), egui::Vec2::splat(14.0)),
            );
            return;
        }
        let latency = self.snapshot.profile_latencies.get(id).copied();
        let label = match latency {
            Some(ProfileLatency::Reachable { milliseconds }) => {
                let text = if display == PingDisplay::Icon {
                    "●".into()
                } else {
                    self.ping_status(ProfileLatency::Reachable { milliseconds })
                };
                egui::RichText::new(text).color(theme::ping_color(ui.visuals(), milliseconds))
            }
            Some(ProfileLatency::TimedOut | ProfileLatency::Unreachable) => {
                egui::RichText::new("×").color(theme::ping_failure_color(ui.visuals()))
            }
            None => egui::RichText::new(self.tr(Key::Ping)),
        };
        let blocked = self.tcp_probe_blocked();
        let button = egui::Button::new(label)
            .frame_when_inactive(false)
            .truncate();
        let response = ui.put(rect, |ui: &mut egui::Ui| {
            ui.add_enabled(blocked.is_none(), button)
        });
        response.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::Role::Button,
                blocked.is_none(),
                self.tr(Key::PingTest),
            )
        });
        if let Some(reason) = blocked {
            response.on_disabled_hover_text(self.tr(reason));
        } else {
            let tooltip = latency.map_or_else(
                || self.tr(Key::PingTest).to_owned(),
                |latency| self.ping_status(latency),
            );
            if response.on_hover_text(tooltip).clicked() {
                self.emit(Intent::ProbeProfile(id.to_owned()));
            }
        }
    }

    fn profile_choice(
        &mut self,
        ui: &mut egui::Ui,
        rect: egui::Rect,
        profile: &ProfileSummary,
        selected: bool,
    ) -> egui::Response {
        let can_select = !self.snapshot.busy
            && self.snapshot.active_mode.is_none()
            && !self.snapshot.cleanup_required;
        let enabled = can_select || selected;
        let response = ui
            .add_enabled_ui(enabled, |ui| {
                ui.interact(rect, ui.make_persistent_id("select"), egui::Sense::click())
            })
            .inner;
        response.widget_info(|| {
            egui::WidgetInfo::selected(egui::Role::Button, enabled, selected, &profile.name)
        });
        if response.clicked() && !selected {
            self.emit(Intent::SelectProfile(profile.profile_uuid.clone()));
        }
        let disabled_reason = if self.snapshot.busy {
            Key::Working
        } else {
            Key::ModeFinishFirst
        };
        response.on_disabled_hover_text(self.tr(disabled_reason))
    }

    fn profile_ping(
        &self,
        ui: &mut egui::Ui,
        rect: egui::Rect,
        profile: &ProfileSummary,
        compact: bool,
        display: PingDisplay,
    ) {
        let latency = self.profile_latency(profile);
        let (width, top) = if compact {
            (49.0, rect.center().y)
        } else {
            (63.0, rect.top())
        };
        let ping_rect = egui::Rect::from_min_max(
            egui::pos2(rect.right() - width, top),
            egui::pos2(rect.right() - 6.0, rect.bottom()),
        );
        if self.profile_probe_pending(profile) {
            egui::Spinner::new().paint_at(
                ui,
                egui::Rect::from_center_size(
                    ping_rect.right_center() - egui::vec2(8.0, 0.0),
                    egui::Vec2::splat(14.0),
                ),
            );
            return;
        }
        ui.put(ping_rect, |ui: &mut egui::Ui| {
            self.ping_indicator(ui, latency, display, egui::Align::Max)
        });
    }

    pub(super) fn selected_profile_ping(
        &self,
        ui: &mut egui::Ui,
        profile: Option<&ProfileSummary>,
    ) {
        if let Some(profile) = profile {
            let provider = profile
                .subscription_uuid
                .as_deref()
                .and_then(|uuid| self.subscription_provider(uuid))
                .map(|provider| &provider.display);
            self.ping_indicator(
                ui,
                self.profile_latency(profile),
                self.group_ping_display(provider),
                egui::Align::Center,
            );
        }
    }

    fn ping_indicator(
        &self,
        ui: &mut egui::Ui,
        latency: Option<ProfileLatency>,
        display: PingDisplay,
        align: egui::Align,
    ) -> egui::Response {
        let description = latency.map_or_else(
            || self.tr(Key::PingNotMeasured).to_owned(),
            |latency| self.ping_status(latency),
        );
        let value = match latency {
            Some(ProfileLatency::Reachable { milliseconds }) => {
                let color = theme::ping_color(ui.visuals(), milliseconds);
                if display == PingDisplay::Icon {
                    return ping_badge(ui, color, false).on_hover_text(description);
                }
                egui::RichText::new(description.as_str())
                    .small()
                    .color(color)
            }
            Some(ProfileLatency::TimedOut | ProfileLatency::Unreachable) => {
                let color = theme::ping_failure_color(ui.visuals());
                let badge = ping_badge(ui, color, true);
                badge.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::Role::Label, ui.is_enabled(), &description)
                });
                return badge.on_hover_text(description);
            }
            None => egui::RichText::new("—").small().weak(),
        };
        ui.add(
            egui::Label::new(value)
                .selectable(false)
                .truncate()
                .halign(align),
        )
        .on_hover_text(description)
    }

    pub(in crate::presentation::widgets) fn profile_ping_button(
        &mut self,
        ui: &mut egui::Ui,
        profile: Option<&ProfileSummary>,
        prominent: bool,
    ) -> bool {
        let pending = profile.is_some_and(|profile| self.profile_probe_pending(profile));
        let label = self.tr(Key::PingTest);
        let mut button = egui::Button::new(label).truncate();
        if pending {
            let spinner = egui::Atom::paint(egui::Vec2::splat(14.0), |ui, args| {
                egui::Spinner::new().paint_at(ui, args.rect);
            });
            button = egui::Button::new((label, spinner)).truncate();
        }
        if prominent {
            button = button.min_size(egui::vec2(
                ui.available_width().min(CONNECTION_CONTROL_WIDTH),
                SELECTOR_HEIGHT,
            ));
        }
        let blocked = self.profile_ping_unavailable(profile);
        let response = ui.add_enabled(blocked.is_none(), button);
        if let Some(reason) = blocked {
            response.on_disabled_hover_text(self.tr(reason));
            return false;
        }
        if let Some(profile) = profile {
            let clicked = response.clicked();
            if let Some(latency) = self.profile_latency(profile) {
                response.on_hover_text(self.ping_status(latency));
            }
            if clicked {
                self.emit(Intent::ProbeProfile(profile.profile_uuid.clone()));
                return true;
            }
        }
        false
    }

    fn profile_ping_unavailable(&self, profile: Option<&ProfileSummary>) -> Option<Key> {
        self.tcp_probe_blocked().or_else(|| {
            if self.snapshot.profiles.is_empty() {
                Some(Key::NoAvailableServers)
            } else {
                profile.is_none().then_some(Key::SelectProfile)
            }
        })
    }

    pub(super) fn tcp_probe_blocked(&self) -> Option<Key> {
        if self.snapshot.busy || self.view.catalog_operation.is_some() {
            Some(Key::Working)
        } else if self.snapshot.cleanup_required {
            Some(Key::CleanupRequired)
        } else if !self.snapshot.tcp_probe_available {
            Some(Key::PingTunUnavailable)
        } else {
            None
        }
    }

    fn profile_latency(&self, profile: &ProfileSummary) -> Option<ProfileLatency> {
        self.snapshot
            .profile_latencies
            .get(&profile.profile_uuid)
            .copied()
    }

    fn profile_probe_pending(&self, profile: &ProfileSummary) -> bool {
        self.probe_pending(&profile.profile_uuid, profile.subscription_uuid.as_deref())
    }

    fn probe_pending(&self, id: &str, subscription_uuid: Option<&str>) -> bool {
        match &self.view.catalog_operation {
            Some((CatalogOperation::ProbeProfile(uuid), _)) => uuid == id,
            Some((CatalogOperation::ProbeGroup(uuid), _)) => uuid
                .as_deref()
                .is_none_or(|uuid| subscription_uuid == Some(uuid)),
            _ => false,
        }
    }

    fn ping_status(&self, latency: ProfileLatency) -> String {
        match latency {
            ProfileLatency::Reachable { milliseconds } => self.message(
                Key::PingMilliseconds,
                &[("time", &milliseconds.to_string())],
            ),
            ProfileLatency::TimedOut => self.tr(Key::PingTimedOut).to_owned(),
            ProfileLatency::Unreachable => self.tr(Key::PingUnreachable).to_owned(),
        }
    }

    fn profile_editor_button(
        &mut self,
        ui: &mut egui::Ui,
        rect: egui::Rect,
        profile: &ProfileSummary,
    ) {
        let button = egui::Button::new("›").frame_when_inactive(false);
        let label = self.tr(Key::EditConfiguration);
        let response = ui.put(rect, button);
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
        if response.on_hover_text(label).clicked() {
            self.view.open_profile_editor(&profile.profile_uuid);
        }
    }
}

fn ping_badge(ui: &mut egui::Ui, color: egui::Color32, cross: bool) -> egui::Response {
    let diameter: f32 = if cross { 18.0 } else { 8.0 };
    let size = egui::vec2(
        ui.available_width(),
        diameter.max(ui.text_style_height(&egui::TextStyle::Small)),
    );
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::hover());
    if cross {
        paint_failure_badge(
            ui.painter(),
            rect.center(),
            color,
            ui.visuals().extreme_bg_color,
        );
    } else {
        ui.painter()
            .circle_filled(rect.center(), diameter * 0.5, color);
    }
    response
}

fn server_identity(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    name: &str,
    subtitle: &str,
    compact: bool,
    trailing_width: f32,
) {
    let (badge_x, badge_size, left_padding) = if compact {
        (16.0, 18.0, 33.0)
    } else {
        (25.0, 24.0, 47.0)
    };
    paint_server_flag(
        ui,
        egui::pos2(rect.left() + badge_x, rect.center().y),
        badge_size,
        name,
    );
    let name = server_name(name);
    let left = rect.left() + left_padding;
    let right = (rect.right() - trailing_width).max(left);
    if compact {
        compact_profile_text(ui, rect, left, right, name, subtitle);
        return;
    }
    let identity = egui::Rect::from_min_max(
        egui::pos2(left, rect.top() + 6.0),
        egui::pos2(right, rect.bottom() - 4.0),
    );
    let mut ui = ui.new_child(
        egui::UiBuilder::new()
            .id_salt("identity")
            .max_rect(identity)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    ui.spacing_mut().item_spacing.y = 2.0;
    ui.add(
        egui::Label::new(egui::RichText::new(name).strong())
            .selectable(false)
            .truncate(),
    )
    .on_hover_text(name);
    ui.add(
        egui::Label::new(egui::RichText::new(subtitle).weak())
            .selectable(false)
            .truncate(),
    )
    .on_hover_text(subtitle);
}

fn compact_profile_text(
    ui: &mut egui::Ui,
    row: egui::Rect,
    left: f32,
    right: f32,
    name: &str,
    subtitle: &str,
) {
    let top = egui::Rect::from_min_max(
        egui::pos2(left, row.top() + 3.0),
        egui::pos2((row.right() - 3.0).max(left), row.center().y),
    );
    let bottom = egui::Rect::from_min_max(
        egui::pos2(left, row.center().y),
        egui::pos2(right, row.bottom() - 2.0),
    );
    let labels = [
        ("name", top, egui::RichText::new(name).strong(), name),
        (
            "protocol",
            bottom,
            egui::RichText::new(subtitle).weak(),
            subtitle,
        ),
    ];
    for (key, rect, text, tooltip) in labels {
        let mut line = ui.new_child(
            egui::UiBuilder::new()
                .id_salt(key)
                .max_rect(rect)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        line.add(egui::Label::new(text).selectable(false).truncate())
            .on_hover_text(tooltip);
    }
}

fn paint_row(ui: &egui::Ui, rect: egui::Rect, selected: bool, response: &egui::Response) {
    let visuals = ui.visuals();
    let highlight = rect.shrink2(egui::vec2(0.0, 2.0));
    if selected || response.hovered() || response.has_focus() {
        let fill = if selected {
            visuals.selection.bg_fill
        } else {
            visuals.widgets.hovered.bg_fill
        };
        ui.painter().rect_filled(highlight, 4, fill);
    }
    if selected {
        ui.painter().rect_filled(
            egui::Rect::from_min_size(highlight.min, egui::vec2(3.0, highlight.height())),
            2,
            ui.visuals().hyperlink_color,
        );
    }
}

pub(super) fn paint_server_flag(ui: &egui::Ui, center: egui::Pos2, size: f32, name: &str) {
    let rect = egui::Rect::from_center_size(center, egui::vec2(size, size * 0.72));
    if paint_flag(ui, rect, name) {
        return;
    }
    let visuals = ui.visuals();
    let painter = ui.painter();
    painter.rect_filled(rect, 4, visuals.widgets.inactive.bg_fill);
    let points = [(-0.21, 0.23), (-0.21, -0.23), (0.22, -0.16), (-0.21, 0.02)];
    let points = points
        .into_iter()
        .map(|(x, y)| center + egui::vec2(x, y) * size)
        .collect();
    painter.add(egui::Shape::line(
        points,
        egui::Stroke::new(1.25, visuals.weak_text_color()),
    ));
}
