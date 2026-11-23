use std::f32::consts::PI;

use vpn_presentation::session::{ConnectionStatus, ProfileSummary};

use super::ViewFrame;
use super::profiles::paint_server_flag;
use crate::view::widgets::connection_controls::CONNECTION_CONTROL_WIDTH;
use crate::view::widgets::controls::{paint_spinner, selector, selector_option};
use crate::view::widgets::flags::server_name;
use crate::view::widgets::layout::ViewportLayout;
use vpn_presentation::localization::Key;
use vpn_presentation::model::{self as viewmodel, Intent};

impl<'a> ViewFrame<'a> {
    pub(in crate::view::widgets) fn connection(&mut self, ui: &mut egui::Ui) {
        let bounds = ui.available_rect_before_wrap();
        if self.layout == ViewportLayout::Mobile || bounds.height() < 460.0 {
            self.compact_connection(ui);
            return;
        }
        let split = bounds.center().x;
        let catalog = egui::Rect::from_min_max(bounds.min, egui::pos2(split, bounds.bottom()));
        let pane = egui::Rect::from_min_max(egui::pos2(split, bounds.top()), bounds.max);
        ui.allocate_rect(bounds, egui::Sense::hover());
        ui.painter()
            .rect_filled(pane, 0, ui.visuals().extreme_bg_color);
        ui.painter().vline(
            split,
            pane.y_range(),
            ui.visuals().widgets.noninteractive.bg_stroke,
        );
        let inset = f32::from(self.layout.content_inset());
        let mut list_ui = pane_ui(ui, "catalog", catalog);
        self.home_catalog(&mut list_ui);
        let mut right_ui = pane_ui(ui, "connection-pane", pane.shrink(inset));
        self.connection_pane(&mut right_ui);
    }

    fn connection_pane(&mut self, ui: &mut egui::Ui) {
        self.connection_mode_row(ui);
        ui.add_space(10.0);
        ui.vertical_centered(|ui| {
            let diameter = (ui.available_height() - 210.0).clamp(100.0, 196.0);
            self.primary_control(ui, diameter);
            ui.add_space(10.0);
            self.connection_status(ui);
        });
        ui.add_space((ui.available_height() - 128.0).max(8.0));
        ui.vertical_centered(|ui| self.selected_server(ui, true));
    }

    fn compact_connection(&mut self, ui: &mut egui::Ui) {
        let layout = self.layout;
        layout.scroll_frame().show(ui, |ui| {
            layout.vertical_scroll(ui, "compact-server-screen", |ui| {
                self.connection_mode_row(ui);
                ui.vertical_centered(|ui| {
                    self.primary_control(ui, ui.available_width().min(76.0));
                    self.connection_status(ui);
                    self.selected_server(ui, false);
                });
                ui.separator();
                self.catalog_header(ui);
                self.catalog_contents(ui);
            });
        });
    }

    fn connection_mode_row(&mut self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.label(self.tr(Key::Mode));
            self.mode_picker(ui);
        });
    }

    fn connection_status(&self, ui: &mut egui::Ui) {
        let connection = if self.snapshot.connection == ConnectionStatus::Failed
            && self.view.tun_start_notice.connection_blocked()
        {
            ConnectionStatus::Disconnected
        } else {
            self.snapshot.connection
        };
        ui.label(
            egui::RichText::new(
                self.tr(viewmodel::state_key(connection, self.snapshot.active_mode)),
            )
            .size(17.0)
            .strong(),
        );
        self.connection_details(ui);
    }

    fn selected_server(&mut self, ui: &mut egui::Ui, show_flag: bool) {
        let profile = self.current_profile();
        if show_flag && let Some(profile) = profile {
            let (badge, _) = ui.allocate_exact_size(egui::vec2(42.0, 34.0), egui::Sense::hover());
            paint_server_flag(ui, badge.center(), 38.0, &profile.name);
        }
        self.selected_server_name(ui, profile);
        self.selected_profile_ping(ui, profile);
        ui.add_space(4.0);
        self.profile_ping_button(ui, profile, true);
    }

    fn selected_server_name(&mut self, ui: &mut egui::Ui, profile: Option<&ProfileSummary>) {
        if let Some(profile) = profile {
            ui.add(
                egui::Label::new(
                    egui::RichText::new(server_name(&profile.name))
                        .size(18.0)
                        .strong(),
                )
                .truncate(),
            )
            .on_hover_text(server_name(&profile.name));
        } else if self.snapshot.profiles.is_empty() {
            ui.weak(self.tr(Key::NoAvailableServers));
        } else {
            self.choose_server(ui);
        }
    }

    fn choose_server(&mut self, ui: &mut egui::Ui) {
        let profiles = &self.snapshot.profiles;
        let mut selection = None;
        let blocked = self.snapshot.busy
            || self.snapshot.cleanup_required
            || self.snapshot.active_mode.is_some();
        ui.add_enabled_ui(!blocked, |ui| {
            selector(
                ui,
                "connection-server",
                CONNECTION_CONTROL_WIDTH,
                self.tr(Key::SelectProfile),
                |ui| {
                    for profile in profiles {
                        if ui
                            .add(selector_option(ui, false, server_name(&profile.name)))
                            .clicked()
                        {
                            selection = Some(profile.profile_uuid.clone());
                            ui.close();
                        }
                    }
                },
            );
        });
        if let Some(uuid) = selection {
            self.emit(Intent::SelectProfile(uuid));
        }
    }

    fn current_profile(&self) -> Option<&'a ProfileSummary> {
        let uuid = self
            .snapshot
            .active_profile_uuid
            .as_ref()
            .or(self.snapshot.selected_profile_uuid.as_ref())?;
        self.snapshot
            .profiles
            .iter()
            .find(|profile| &profile.profile_uuid == uuid)
    }

    fn primary_control(&mut self, ui: &mut egui::Ui, diameter: f32) {
        let (key, command) = viewmodel::primary_action(self.snapshot, self.saving_mode);
        let enabled = command.is_some();
        let action = self.tr(key);
        let response = ui.add_enabled(
            enabled,
            egui::Button::new("")
                .frame(false)
                .min_size(egui::Vec2::splat(diameter)),
        );
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, action));
        draw_power(
            ui,
            &response,
            self.snapshot.connection,
            diameter,
            self.saving_mode,
        );
        let response = response
            .on_hover_text(action)
            .on_disabled_hover_text(self.tr(self.disabled_action_reason(key)));
        if response.clicked()
            && let Some(intent) = command
        {
            self.emit(intent);
        }
    }

    fn disabled_action_reason(&self, action: Key) -> Key {
        if action != Key::Connect {
            return action;
        }
        if self.saving_mode {
            Key::ModeSaving
        } else if self.snapshot.selected_profile_uuid.is_none() {
            Key::SelectProfile
        } else if self.snapshot.busy {
            Key::Working
        } else {
            Key::ModeUnavailable
        }
    }
}

fn pane_ui(ui: &mut egui::Ui, id: &'static str, bounds: egui::Rect) -> egui::Ui {
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .id_salt(id)
            .max_rect(bounds)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    child.set_clip_rect(ui.clip_rect().intersect(bounds));
    child
}

fn draw_power(
    ui: &egui::Ui,
    button: &egui::Response,
    status: ConnectionStatus,
    diameter: f32,
    saving_mode: bool,
) {
    let painter = ui.painter();
    let center = button.rect.center();
    let visuals = ui.visuals();
    let active = matches!(
        status,
        ConnectionStatus::Connected | ConnectionStatus::Connecting
    );
    let idle_color = visuals.weak_text_color();
    let accent = if active || button.enabled() {
        visuals.hyperlink_color
    } else {
        idle_color
    };
    let inner = visuals.widgets.inactive.bg_fill;
    let outer = if active {
        visuals.selection.bg_fill
    } else {
        inner.lerp_to_gamma(idle_color, 0.25)
    };
    let highlighted = button.has_focus() || button.hovered();
    let ring_accent = if active { accent } else { idle_color };
    let ring = if highlighted {
        accent
    } else {
        ring_accent.gamma_multiply(0.55)
    };
    let scale = diameter / 146.0;
    painter.circle_stroke(center, 72.0 * scale, egui::Stroke::new(1.0, ring));
    painter.circle_filled(center, 58.0 * scale, outer);
    painter.circle_filled(center, 49.0 * scale, inner);
    if saving_mode
        || matches!(
            status,
            ConnectionStatus::Connecting | ConnectionStatus::Disconnecting
        )
    {
        paint_spinner(ui, center, (diameter * 0.23).clamp(14.0, 34.0));
        return;
    }
    let symbol_scale = scale.clamp(0.74, 1.25);
    let stroke = egui::Stroke::new(3.5 * symbol_scale, accent);
    let arc = (0..=24)
        .map(|index| {
            let angle = -PI / 4.0 + index as f32 * 1.5 * PI / 24.0;
            center + egui::vec2(angle.cos(), angle.sin()) * 13.0 * symbol_scale
        })
        .collect();
    painter.add(egui::Shape::line(arc, stroke));
    painter.line_segment(
        [center + egui::vec2(0.0, -17.0 * symbol_scale), center],
        stroke,
    );
}
