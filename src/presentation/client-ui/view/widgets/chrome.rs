use super::ViewFrame;
use super::controls::paint_spinner;
use super::icons::Icon;
use super::layout::ViewportLayout;
use crate::view::view_model::{Screen, SettingsPage};
use vpn_presentation::localization::Key;
use vpn_presentation::session::ConnectionStatus;

const NAV_COLLAPSED: f32 = 70.0;
const NAV_EXPANDED: f32 = 166.0;
const TABLET_RAIL_WIDTH: f32 = 56.0;
const NAV_ROW_HEIGHT: f32 = 40.0;
const NAV_ROWS: f32 = NAV_LINKS.len() as f32 + 3.0;
const MOBILE_BAR_HEIGHT: f32 = 54.0;
const HEADING_SIZE: f32 = 24.0;

const NAV_LINKS: [(Screen, Icon); 4] = [
    (Screen::Home, Icon::Servers),
    (Screen::Settings(SettingsPage::Index), Icon::Settings),
    (Screen::Statistics, Icon::Statistics),
    (Screen::Settings(SettingsPage::Logs), Icon::Logs),
];

#[derive(Clone, Copy)]
enum NavStyle {
    Rail { expanded: bool },
    Bottom { width: f32 },
}

impl ViewFrame<'_> {
    pub(super) fn navigation(&mut self, ui: &mut egui::Ui) {
        match self.layout {
            ViewportLayout::Mobile => self.bottom_navigation(ui),
            ViewportLayout::Tablet | ViewportLayout::Desktop => self.sidebar(ui),
        }
    }

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        let can_expand = self.layout == ViewportLayout::Desktop;
        let expanded = can_expand && self.view.sidebar_expanded;
        let width = match self.layout {
            ViewportLayout::Tablet => TABLET_RAIL_WIDTH,
            _ if expanded => NAV_EXPANDED,
            _ => NAV_COLLAPSED,
        };
        let style = NavStyle::Rail { expanded };
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
                if can_expand {
                    self.sidebar_toggle(ui, expanded);
                }
                self.add_navigation_button(ui, style);
                for (screen, icon) in NAV_LINKS {
                    self.navigation_link(ui, screen, icon, style);
                }
                ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
                    self.navigation_link(
                        ui,
                        Screen::Settings(SettingsPage::About),
                        Icon::About,
                        style,
                    );
                });
            });
    }

    fn sidebar_toggle(&mut self, ui: &mut egui::Ui, expanded: bool) {
        let (icon, label) = if expanded {
            (Icon::Collapse, Key::CollapseSidebar)
        } else {
            (Icon::Expand, Key::ExpandSidebar)
        };
        if nav_button(
            ui,
            Some(icon),
            self.tr(label),
            NavStyle::Rail { expanded: false },
            false,
        )
        .clicked()
        {
            self.view.sidebar_expanded = !expanded;
        }
    }

    fn bottom_navigation(&mut self, ui: &mut egui::Ui) {
        let fill = ui.visuals().extreme_bg_color;
        egui::Panel::bottom("mobile-navigation")
            .exact_size(MOBILE_BAR_HEIGHT)
            .resizable(false)
            .show_separator_line(false)
            .frame(
                egui::Frame::new()
                    .fill(fill)
                    .inner_margin(egui::Margin::same(4)),
            )
            .show(ui, |ui| {
                ui.spacing_mut().interact_size.y = MOBILE_BAR_HEIGHT - 8.0;
                ui.spacing_mut().item_spacing.x = 0.0;
                let style = NavStyle::Bottom {
                    width: ui.available_width() / (NAV_LINKS.len() + 2) as f32,
                };
                ui.horizontal(|ui| {
                    self.add_navigation_button(ui, style);
                    for (screen, icon) in NAV_LINKS {
                        self.navigation_link(ui, screen, icon, style);
                    }
                    self.navigation_link(
                        ui,
                        Screen::Settings(SettingsPage::About),
                        Icon::About,
                        style,
                    );
                });
            });
    }

    fn add_navigation_button(&mut self, ui: &mut egui::Ui, style: NavStyle) {
        let busy = self.snapshot.busy || self.saving_mode;
        let icon = (!self.show_sidebar_progress()).then_some(Icon::Add);
        let add_button = ui
            .add_enabled_ui(!busy, |ui| {
                nav_button(ui, icon, self.tr(Key::Add), style, false)
            })
            .inner
            .on_disabled_hover_text(self.tr(Key::Working));
        let popup = egui::Popup::menu(&add_button);
        let popup = if matches!(style, NavStyle::Bottom { .. }) {
            popup.align(egui::RectAlign::TOP_START)
        } else {
            popup
        };
        popup.show(|ui| self.add_menu(ui));
    }

    fn show_sidebar_progress(&self) -> bool {
        let home_action_pending = self.view.screen == Screen::Home
            && (self.view.subscription_refresh.is_some()
                || self.saving_mode
                || matches!(
                    self.snapshot.connection,
                    ConnectionStatus::Connecting | ConnectionStatus::Disconnecting
                ));
        (self.snapshot.busy || self.saving_mode)
            && !home_action_pending
            && self
                .view
                .configuration
                .as_ref()
                .is_none_or(|draft| draft.pending.is_none())
    }

    fn navigation_link(&mut self, ui: &mut egui::Ui, screen: Screen, icon: Icon, style: NavStyle) {
        let active = self.view.screen == screen
            || screen == Screen::Settings(SettingsPage::Index)
                && matches!(self.view.screen, Screen::Settings(page) if !matches!(page, SettingsPage::Logs | SettingsPage::About));
        let label = if matches!(style, NavStyle::Bottom { .. })
            && screen == Screen::Settings(SettingsPage::About)
        {
            Key::AboutCompact
        } else {
            screen.key()
        };
        if nav_button(ui, Some(icon), self.tr(label), style, active).clicked() {
            self.view.navigate(screen);
        }
    }

    pub(super) fn header(&mut self, ui: &mut egui::Ui) {
        let screen = self.view.screen;
        let layout = match self.layout {
            ViewportLayout::Mobile => egui::Layout::top_down(egui::Align::Min),
            ViewportLayout::Tablet => {
                egui::Layout::left_to_right(egui::Align::Center).with_main_wrap(true)
            }
            ViewportLayout::Desktop => egui::Layout::left_to_right(egui::Align::Center),
        };
        ui.allocate_ui_with_layout(
            egui::vec2(ui.available_width(), HEADING_SIZE),
            layout,
            |ui| {
                if let Screen::Settings(page) = screen {
                    self.settings_crumbs(ui, page);
                } else {
                    title(ui, self.tr(screen.key()), false);
                }
            },
        );
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
        let mobile = self.layout == ViewportLayout::Mobile;
        let text = if mobile {
            egui::RichText::new(format!("‹ {label}")).size(15.0)
        } else {
            egui::RichText::new(label).size(HEADING_SIZE)
        };
        if ui
            .add(egui::Button::new(text).frame(false).truncate())
            .on_hover_text(label)
            .clicked()
        {
            self.view.open(screen);
        }
        if !mobile {
            ui.label("›");
        }
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
    icon: Option<Icon>,
    label: &str,
    style: NavStyle,
    active: bool,
) -> egui::Response {
    let width = match style {
        NavStyle::Rail { .. } => ui.available_width(),
        NavStyle::Bottom { width } => width,
    };
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
    let icon_center = match style {
        NavStyle::Rail { expanded: true } => egui::pos2(response.rect.left() + 22.0, center.y),
        NavStyle::Rail { expanded: false } => center,
        NavStyle::Bottom { .. } => center - egui::vec2(0.0, 7.0),
    };
    if let Some(icon) = icon {
        icon.paint(painter, icon_center, color);
    } else {
        paint_spinner(ui, icon_center, 16.0);
    }
    match style {
        NavStyle::Rail { expanded: true } => {
            painter.text(
                icon_center + egui::vec2(25.0, 0.0),
                egui::Align2::LEFT_CENTER,
                label,
                egui::FontId::proportional(14.0),
                color,
            );
        }
        NavStyle::Rail { expanded: false } => {}
        NavStyle::Bottom { .. } => paint_mobile_nav_label(ui, response.rect, label, color),
    }
    if active {
        let stroke = egui::Stroke::new(3.0, ui.visuals().hyperlink_color);
        match style {
            NavStyle::Rail { .. } => {
                let left = response.rect.left() + 1.5;
                painter.vline(left, center.y - 8.0..=center.y + 8.0, stroke);
            }
            NavStyle::Bottom { .. } => {
                painter.hline(
                    center.x - 8.0..=center.x + 8.0,
                    response.rect.top() + 1.5,
                    stroke,
                );
            }
        }
    }
    response.on_hover_text(label)
}

fn paint_mobile_nav_label(ui: &egui::Ui, bounds: egui::Rect, label: &str, color: egui::Color32) {
    let mut job = egui::text::LayoutJob::simple_singleline(
        label.to_owned(),
        egui::FontId::proportional(10.0),
        color,
    );
    job.wrap.max_width = (bounds.width() - 4.0).max(0.0);
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    let painter = ui.painter();
    let galley = painter.layout_job(job);
    let center = bounds.center() + egui::vec2(0.0, 12.0);
    painter.galley(center - galley.size() * 0.5, galley, color);
}
