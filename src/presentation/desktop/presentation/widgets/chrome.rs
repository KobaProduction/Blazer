use eframe::egui;

use super::ViewFrame;
use super::icons::Icon;
use crate::presentation::view_model::{Screen, SettingsPage};
use vpn_presentation::localization::Key;

pub(super) const CONTENT_INSET: i8 = 22;
const NAV_COLLAPSED: f32 = 70.0;
const NAV_EXPANDED: f32 = 166.0;
const NAV_ROW_HEIGHT: f32 = 40.0;
const NAV_ROWS: f32 = NAV_LINKS.len() as f32 + 3.0;
const MIN_WORKSPACE_WIDTH: f32 = 340.0;
const HEADING_SIZE: f32 = 24.0;

const NAV_LINKS: [(Screen, Icon); 4] = [
    (Screen::Home, Icon::Servers),
    (Screen::Settings(SettingsPage::Index), Icon::Settings),
    (Screen::Statistics, Icon::Statistics),
    (Screen::Settings(SettingsPage::Logs), Icon::Logs),
];

impl ViewFrame<'_> {
    pub(super) fn navigation(&mut self, ui: &mut egui::Ui) {
        let can_expand = ui.available_width() >= NAV_EXPANDED + MIN_WORKSPACE_WIDTH;
        let expanded = self.view.sidebar_expanded && can_expand;
        let width = if expanded {
            NAV_EXPANDED
        } else {
            NAV_COLLAPSED
        };
        let fill = ui.visuals().extreme_bg_color;
        egui::Panel::left("desktop-sidebar")
            .exact_size(width)
            .resizable(false)
            .show_separator_line(false)
            .frame(
                egui::Frame::new()
                    .fill(fill)
                    .inner_margin(egui::Margin::same(5)),
            )
            .show(ui, |ui| {
                let spacing = ui.spacing().item_spacing.y;
                let height = ((ui.available_height() - spacing * (NAV_ROWS - 1.0)) / NAV_ROWS)
                    .clamp(20.0, NAV_ROW_HEIGHT);
                ui.spacing_mut().interact_size.y = height;
                self.sidebar_items(ui, expanded, can_expand);
                ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
                    self.sidebar_link(
                        ui,
                        Screen::Settings(SettingsPage::About),
                        Icon::About,
                        expanded,
                    );
                });
            });
    }

    fn sidebar_items(&mut self, ui: &mut egui::Ui, expanded: bool, can_expand: bool) {
        let (icon, label) = if expanded {
            (Icon::Collapse, Key::CollapseSidebar)
        } else {
            (Icon::Expand, Key::ExpandSidebar)
        };
        if ui
            .add_enabled_ui(can_expand, |ui| {
                nav_button(ui, icon, self.tr(label), false, false)
            })
            .inner
            .clicked()
        {
            self.view.sidebar_expanded = !expanded;
        }
        let add_button = nav_button(ui, Icon::Add, self.tr(Key::Add), expanded, false);
        egui::Popup::menu(&add_button).show(|ui| self.add_menu(ui));
        for (screen, icon) in NAV_LINKS {
            self.sidebar_link(ui, screen, icon, expanded);
        }
    }

    fn sidebar_link(&mut self, ui: &mut egui::Ui, screen: Screen, icon: Icon, expanded: bool) {
        let active = self.view.screen == screen
            || screen == Screen::Settings(SettingsPage::Index)
                && matches!(self.view.screen, Screen::Settings(page) if !matches!(page, SettingsPage::Logs | SettingsPage::About));
        if nav_button(ui, icon, self.tr(screen.key()), expanded, active).clicked() {
            self.view.navigate(screen);
        }
    }

    pub(super) fn header(&mut self, ui: &mut egui::Ui) {
        let screen = self.view.screen;
        ui.horizontal(|ui| {
            if let Screen::Settings(page) = screen {
                self.settings_crumbs(ui, page);
            } else {
                title(ui, self.tr(screen.key()), false);
            }
        });
        ui.add_space(10.0);
    }

    fn settings_crumbs(&mut self, ui: &mut egui::Ui, page: SettingsPage) {
        use SettingsPage::{About, Index, Logs, RoutingProfile, Subscription, SubscriptionRouting};
        let screen = Screen::Settings(page);
        match page {
            Index | Logs | About => title(ui, self.tr(screen.key()), false),
            SubscriptionRouting => self.subscription_crumbs(ui),
            RoutingProfile => self.routing_profile_crumbs(ui),
            _ => {
                let root = if page == Subscription && self.view.previous() == Some(Screen::Home) {
                    Screen::Home
                } else {
                    Screen::Settings(Index)
                };
                self.crumb_link(ui, root, self.tr(root.key()));
                if page == SettingsPage::ExcludedRoutes {
                    let advanced = Screen::Settings(SettingsPage::Advanced);
                    self.crumb_link(ui, advanced, self.tr(advanced.key()));
                }
                title(ui, self.tr(screen.key()), true);
            }
        }
    }

    fn subscription_title(&self) -> &str {
        self.view
            .subscription_routing_uuid
            .as_deref()
            .and_then(|uuid| {
                self.snapshot
                    .subscriptions
                    .iter()
                    .find(|subscription| subscription.subscription_uuid == uuid)
            })
            .map_or(self.tr(Key::Routing), |subscription| {
                subscription.title.as_str()
            })
    }

    fn subscription_crumbs(&mut self, ui: &mut egui::Ui) {
        self.crumb_link(ui, Screen::Home, self.tr(Key::Servers));
        title(ui, self.subscription_title(), true);
    }

    fn routing_profile_crumbs(&mut self, ui: &mut egui::Ui) {
        let parent = self.view.desktop_settings.routing.profile_parent();
        if parent == SettingsPage::SubscriptionRouting {
            self.crumb_link(ui, Screen::Home, self.tr(Key::Servers));
            let name = self.subscription_title().to_owned();
            self.crumb_link(ui, Screen::Settings(parent), &name);
        } else {
            self.crumb_link(
                ui,
                Screen::Settings(SettingsPage::Index),
                self.tr(Key::Settings),
            );
            self.crumb_link(ui, Screen::Settings(parent), self.tr(parent.key()));
        }
        title(
            ui,
            self.view
                .desktop_settings
                .routing
                .profile_title(self.view.locale),
            true,
        );
    }

    fn crumb_link(&mut self, ui: &mut egui::Ui, screen: Screen, label: &str) {
        if ui
            .add(egui::Button::new(egui::RichText::new(label).size(HEADING_SIZE)).frame(false))
            .clicked()
        {
            self.view.open(screen);
        }
        ui.label("›");
    }

    pub(super) fn error_banner(&mut self, ui: &mut egui::Ui) {
        if let Some(error) = self.error
            && self.view.configuration.is_none()
            && self.view.dismissed_error != Some((self.snapshot.revision, error))
        {
            let message = error.text(self.view.locale);
            let dismissed = super::controls::feedback(ui, &message, Some(self.tr(Key::Close)));
            if dismissed {
                self.view.dismissed_error = Some((self.snapshot.revision, error));
            }
            ui.add_space(8.0);
        }
        if self.show_busy_label() {
            ui.label(self.tr(Key::Working));
        }
    }

    pub(super) fn show_busy_label(&self) -> bool {
        self.snapshot.busy && self.refreshing_subscription().is_none()
    }
}

fn title(ui: &mut egui::Ui, label: &str, current: bool) {
    let mut text = egui::RichText::new(label).size(HEADING_SIZE).strong();
    if current {
        text = text.color(ui.visuals().hyperlink_color);
    }
    ui.add(egui::Label::new(text).truncate())
        .on_hover_text(label);
}

fn nav_button(
    ui: &mut egui::Ui,
    icon: Icon,
    label: &str,
    expanded: bool,
    active: bool,
) -> egui::Response {
    let width = ui.available_width();
    let mut button = egui::Button::new("").frame_when_inactive(active);
    if active {
        button = button.fill(ui.visuals().selection.bg_fill);
    }
    let response = ui.add_sized([width, ui.spacing().interact_size.y], button);
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::Role::Button, response.enabled(), active, label)
    });
    let painter = ui.painter();
    let center = response.rect.center();
    let color = if active {
        ui.visuals().hyperlink_color
    } else {
        ui.style().interact(&response).fg_stroke.color
    };
    let icon_x = if expanded {
        response.rect.left() + 22.0
    } else {
        center.x
    };
    icon.paint(painter, egui::pos2(icon_x, center.y), color);
    if expanded {
        painter.text(
            egui::pos2(icon_x + 25.0, center.y),
            egui::Align2::LEFT_CENTER,
            label,
            egui::FontId::proportional(14.0),
            color,
        );
    }
    if active {
        let left = response.rect.left() + 1.5;
        painter.line_segment(
            [
                egui::pos2(left, center.y - 8.0),
                egui::pos2(left, center.y + 8.0),
            ],
            egui::Stroke::new(3.0, ui.visuals().hyperlink_color),
        );
    }
    response.on_hover_text(label)
}
