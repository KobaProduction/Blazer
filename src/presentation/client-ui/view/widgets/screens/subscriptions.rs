use std::collections::HashMap;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Local};
use vpn_presentation::session::{
    ProfileLatency, ProfileSummary, ProviderDisplay, ProviderNotice, ProviderNoticeColor,
    ProviderSort, ProviderSubscriptionSummary, SubscriptionSummary, SubscriptionUsage,
    UnavailableProfile,
};

use super::ViewFrame;
use crate::view::settings_state::{PingDisplay, ServerSort};
use crate::view::theme;
use crate::view::view_model::ConfigurationKind;
use crate::view::widgets::controls::{SELECTOR_HEIGHT, more_button, paint_spinner, roomy_input};
use crate::view::widgets::icons::{Icon, paint_failure_badge};
use vpn_presentation::localization::{Key, Locale};
use vpn_presentation::model::{self as viewmodel, Intent, Removal};

enum ServerRow<'a> {
    Available(&'a ProfileSummary),
    Unavailable(&'a UnavailableProfile),
}

impl ServerRow<'_> {
    fn name(&self, locale: Locale) -> &str {
        match self {
            Self::Available(profile) => &profile.name,
            Self::Unavailable(profile) => viewmodel::unavailable_profile_name(locale, profile),
        }
    }
}

type ServerGroups<'a> = HashMap<Option<&'a str>, Vec<ServerRow<'a>>>;
const TOOL_GAP: f32 = 8.0;
const TOOL_SIZE: f32 = 34.0;
const TOOL_SPINNER_SIZE: f32 = 15.0;

impl<'a> ViewFrame<'a> {
    pub(super) fn home_catalog(&mut self, ui: &mut egui::Ui) {
        let layout = self.layout;
        layout.scroll_frame().show(ui, |ui| {
            layout.scroll_header(ui, |ui| self.catalog_header(ui));
            layout.vertical_scroll(ui, "server-catalog", |ui| self.catalog_contents(ui));
        });
    }

    pub(super) fn catalog_header(&mut self, ui: &mut egui::Ui) {
        ui.label(
            egui::RichText::new(self.tr(Key::Servers))
                .size(26.0)
                .strong(),
        );
        ui.add_space(8.0);
        self.catalog_toolbar(ui);
        ui.add_space(8.0);
        self.error_banner(ui);
    }

    pub(super) fn catalog_contents(&mut self, ui: &mut egui::Ui) {
        let query = self.view.search.trim().to_lowercase();
        let groups = self.server_groups(&query);
        self.catalog_groups(ui, &groups, !query.is_empty());
    }

    fn catalog_toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = TOOL_GAP;
            let hint = self.tr(Key::SearchServers);
            let show_ping = ui.available_width() >= 185.0;
            let action_count = if show_ping { 2.0 } else { 1.0 };
            let search_width =
                (ui.available_width() - action_count * (TOOL_SIZE + TOOL_GAP)).max(1.0);
            ui.add(
                roomy_input(&mut self.view.search, search_width)
                    .id_salt("servers-search")
                    .hint_text(hint)
                    .suffix(egui::Atom::paint(egui::Vec2::splat(16.0), |ui, args| {
                        let stroke = egui::Stroke::new(1.25, ui.visuals().weak_text_color());
                        let center = args.rect.center() - egui::Vec2::splat(1.0);
                        ui.painter().circle_stroke(center, 5.0, stroke);
                        ui.painter().line_segment(
                            [
                                center + egui::Vec2::splat(3.5),
                                center + egui::Vec2::splat(7.0),
                            ],
                            stroke,
                        );
                    })),
            );
            if show_ping {
                self.ping_icon(ui, Key::PingAll, None);
            }
            catalog_menu_button(ui, self.tr(Key::ServerCatalogActions), |ui| {
                self.catalog_menu(ui)
            });
        });
    }

    fn catalog_menu(&mut self, ui: &mut egui::Ui) {
        self.add_menu(ui);
        ui.separator();
        self.ping_menu_item(ui, Key::PingAll, None);
        let has_manual_profiles = self
            .snapshot
            .profiles
            .iter()
            .any(|profile| profile.subscription_uuid.is_none());
        let mut collapsible = self
            .snapshot
            .subscriptions
            .iter()
            .map(|subscription| subscription.subscription_uuid.as_str())
            .filter(|uuid| !self.keep_subscription_expanded(uuid))
            .peekable();
        if !has_manual_profiles && collapsible.peek().is_none() {
            return;
        }
        let manual_closed = !has_manual_profiles || self.view.manual_group_collapsed;
        let all_closed = manual_closed
            && collapsible.all(|uuid| self.view.collapsed_subscriptions.contains(uuid));
        let key = if all_closed {
            Key::ShowGroups
        } else {
            Key::HideGroups
        };
        if ui.button(self.tr(key)).clicked() {
            self.set_groups_collapsed(!all_closed);
            ui.close();
        }
    }

    fn set_groups_collapsed(&mut self, collapsed: bool) {
        self.view.manual_group_collapsed = collapsed;
        self.view.collapsed_subscriptions.clear();
        if collapsed {
            for subscription in &self.snapshot.subscriptions {
                let uuid = &subscription.subscription_uuid;
                if !self.keep_subscription_expanded(uuid) {
                    self.view.collapsed_subscriptions.insert(uuid.clone());
                }
            }
        }
    }

    pub(super) fn subscription_provider(
        &self,
        uuid: &str,
    ) -> Option<&'a ProviderSubscriptionSummary> {
        self.snapshot
            .provider_subscriptions
            .iter()
            .find(|provider| provider.subscription_uuid == uuid)
    }

    fn keep_subscription_expanded(&self, uuid: &str) -> bool {
        self.subscription_provider(uuid)
            .is_some_and(|provider| provider.display.keep_expanded)
    }

    fn subscription_pinned(&self, uuid: &str) -> bool {
        self.view
            .pinned_subscriptions
            .get(uuid)
            .copied()
            .or_else(|| self.subscription_provider(uuid)?.display.pin)
            .unwrap_or(false)
    }

    fn server_groups(&self, query: &str) -> ServerGroups<'a> {
        let locale = self.view.locale;
        let available = self.snapshot.profiles.iter().map(|profile| {
            (
                profile.subscription_uuid.as_deref(),
                ServerRow::Available(profile),
            )
        });
        let unavailable = self
            .snapshot
            .unavailable_profiles
            .iter()
            .flat_map(|(uuid, profiles)| {
                profiles
                    .iter()
                    .map(move |profile| (Some(uuid.as_str()), ServerRow::Unavailable(profile)))
            });
        let mut groups = ServerGroups::new();
        for (group, row) in available.chain(unavailable) {
            if query.is_empty() || row.name(locale).to_lowercase().contains(query) {
                groups.entry(group).or_default().push(row);
            }
        }
        for (uuid, group) in &mut groups {
            match self.group_sort(*uuid) {
                ServerSort::Original => {}
                ServerSort::Alphabetical => {
                    group.sort_by_cached_key(|row| row.name(locale).to_lowercase());
                }
                ServerSort::Ping => group.sort_by_key(|row| self.server_ping_order(row)),
            }
        }
        groups
    }

    fn group_sort(&self, uuid: Option<&str>) -> ServerSort {
        if let Some(sort) = self
            .view
            .desktop_settings
            .subscriptions
            .server_sort_override
        {
            return sort;
        }
        match uuid.and_then(|uuid| self.subscription_provider(uuid)?.display.sort) {
            Some(ProviderSort::Original) => ServerSort::Original,
            Some(ProviderSort::Latency) => ServerSort::Ping,
            Some(ProviderSort::Alphabetical) => ServerSort::Alphabetical,
            None => ServerSort::default(),
        }
    }

    pub(super) fn group_ping_display(&self, provider: Option<&ProviderDisplay>) -> PingDisplay {
        if let Some(display) = self.view.desktop_settings.ping.display_override {
            return display;
        }
        match provider.and_then(|display| display.ping_icon) {
            Some(true) => PingDisplay::Icon,
            Some(false) => PingDisplay::Time,
            None => PingDisplay::default(),
        }
    }

    fn server_ping_order(&self, row: &ServerRow<'_>) -> (u8, u64) {
        let profile_uuid = match row {
            ServerRow::Available(profile) => Some(profile.profile_uuid.as_str()),
            ServerRow::Unavailable(profile) => profile.probe_id.as_deref(),
        };
        let Some(profile_uuid) = profile_uuid else {
            return (3, 0);
        };
        match self.snapshot.profile_latencies.get(profile_uuid) {
            Some(ProfileLatency::Reachable { milliseconds }) => (0, *milliseconds),
            Some(_) => (1, 0),
            None => (2, 0),
        }
    }

    fn catalog_groups(&mut self, ui: &mut egui::Ui, groups: &ServerGroups<'_>, searching: bool) {
        if searching && groups.is_empty() {
            ui.weak(self.tr(Key::NoMatchingServers));
            return;
        }
        let manual = groups.get(&None).map(Vec::as_slice).unwrap_or_default();
        if groups.is_empty() && self.snapshot.subscriptions.is_empty() {
            self.empty_catalog(ui);
        } else if !manual.is_empty() {
            self.manual_group(ui, manual);
        }
        let snapshot = self.snapshot;
        for pinned in [true, false] {
            for subscription in &snapshot.subscriptions {
                if self.subscription_pinned(&subscription.subscription_uuid) != pinned {
                    continue;
                }
                let profiles = groups
                    .get(&Some(subscription.subscription_uuid.as_str()))
                    .map(Vec::as_slice)
                    .unwrap_or_default();
                if !searching || !profiles.is_empty() {
                    self.subscription_group(ui, subscription, profiles);
                }
            }
        }
    }

    fn empty_catalog(&mut self, ui: &mut egui::Ui) {
        catalog_group_frame(ui).show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.label(self.tr(Key::NoProfiles));
            let kind = ConfigurationKind::Configuration;
            if ui
                .add_enabled(!self.snapshot.busy, egui::Button::new(self.tr(kind.key())))
                .clicked()
            {
                self.view.open_configuration(kind);
            }
        });
    }

    fn manual_group(&mut self, ui: &mut egui::Ui, profiles: &[ServerRow<'_>]) {
        ui.push_id("manual-servers", |ui| {
            catalog_group_frame(ui).show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                let closed = self.view.manual_group_collapsed;
                ui.horizontal(|ui| {
                    if self.group_toggle(ui, closed) {
                        self.view.manual_group_collapsed = !closed;
                    }
                    ui.add(
                        egui::Label::new(egui::RichText::new(self.tr(Key::ServerList)).strong())
                            .truncate(),
                    );
                });
                if !closed {
                    self.catalog_rows(ui, profiles, self.group_ping_display(None), None);
                }
            });
        });
    }

    fn subscription_group(
        &mut self,
        ui: &mut egui::Ui,
        subscription: &SubscriptionSummary,
        profiles: &[ServerRow<'_>],
    ) {
        let uuid = &subscription.subscription_uuid;
        let provider = self.subscription_provider(uuid);
        let display = provider.map(|provider| &provider.display);
        let subtitle = self.subscription_caption(provider);
        ui.push_id(("subscription", uuid), |ui| {
            catalog_group_frame(ui).show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                let keep_expanded = display.is_some_and(|display| display.keep_expanded);
                let closed = !keep_expanded && self.view.collapsed_subscriptions.contains(uuid);
                self.subscription_heading(
                    ui,
                    subscription,
                    subtitle.as_deref(),
                    closed,
                    keep_expanded,
                );
                if !closed {
                    if provider.is_some_and(|provider| provider.last_update_used_fallback) {
                        ui.weak(self.tr(Key::FallbackUsed));
                    }
                    if let Some(display) = display {
                        self.provider_banner(ui, uuid, display);
                    }
                    self.subscription_usage(ui, uuid, display);
                    if let Some(display) = display {
                        self.provider_announcement(ui, display);
                    }
                    self.catalog_rows(ui, profiles, self.group_ping_display(display), Some(uuid));
                }
            });
        });
    }

    fn subscription_heading(
        &mut self,
        ui: &mut egui::Ui,
        subscription: &SubscriptionSummary,
        subtitle: Option<&str>,
        closed: bool,
        keep_expanded: bool,
    ) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = TOOL_GAP;
            let uuid = &subscription.subscription_uuid;
            if ui
                .add_enabled_ui(!keep_expanded, |ui| self.group_toggle(ui, closed))
                .inner
            {
                if closed {
                    self.view.collapsed_subscriptions.remove(uuid);
                } else {
                    self.view.collapsed_subscriptions.insert(uuid.clone());
                }
            }
            let show_ping = ui.available_width() >= 190.0;
            let count = if show_ping { 3.0 } else { 2.0 };
            let actions = count * TOOL_SIZE + (count - 1.0) * TOOL_GAP;
            let width = (ui.available_width() - actions - TOOL_GAP).max(0.0);
            ui.allocate_ui_with_layout(
                egui::vec2(width, SELECTOR_HEIGHT),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    ui.set_min_width(width);
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 1.0;
                        self.catalog_subscription_title(ui, subscription);
                        if let Some(subtitle) = subtitle {
                            ui.add(
                                egui::Label::new(egui::RichText::new(subtitle).small().weak())
                                    .truncate(),
                            )
                            .on_hover_text(subtitle);
                        }
                    });
                },
            );
            ui.allocate_ui_with_layout(
                egui::vec2(actions, TOOL_SIZE),
                egui::Layout::right_to_left(egui::Align::Center),
                |ui| self.subscription_actions(ui, subscription, show_ping),
            );
        });
    }

    fn catalog_subscription_title(&self, ui: &mut egui::Ui, subscription: &SubscriptionSummary) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            if self.subscription_pinned(&subscription.subscription_uuid) {
                let (rect, response) =
                    ui.allocate_exact_size(egui::Vec2::splat(14.0), egui::Sense::hover());
                let stroke = egui::Stroke::new(1.3, ui.visuals().hyperlink_color);
                paint_pin(ui.painter(), rect.center(), stroke);
                response.on_hover_text(self.tr(Key::SubscriptionPinned));
            }
            ui.add(egui::Label::new(egui::RichText::new(&subscription.title).strong()).truncate())
                .on_hover_text(&subscription.title);
        });
    }

    fn catalog_rows(
        &mut self,
        ui: &mut egui::Ui,
        profiles: &[ServerRow<'_>],
        ping: PingDisplay,
        subscription_uuid: Option<&str>,
    ) {
        ui.spacing_mut().item_spacing.y = 0.0;
        for (index, profile) in profiles.iter().enumerate() {
            if index > 0 {
                ui.add(egui::Separator::default().spacing(0.0));
            }
            match profile {
                ServerRow::Available(profile) => self.profile_row(ui, profile, ping),
                ServerRow::Unavailable(profile) => {
                    ui.push_id(("unavailable", index), |ui| {
                        self.unavailable_profile_row(ui, profile, ping, subscription_uuid)
                    });
                }
            }
        }
    }

    fn group_toggle(&self, ui: &mut egui::Ui, closed: bool) -> bool {
        let key = if closed {
            Key::ShowGroup
        } else {
            Key::HideGroup
        };
        let label = self.tr(key);
        let button = ui.add_sized(
            [26.0, TOOL_SIZE],
            egui::Button::new("").frame_when_inactive(false),
        );
        button
            .widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), label));
        paint_chevron(ui, button.rect.center(), closed);
        button.on_hover_text(label).clicked()
    }

    fn subscription_actions(
        &mut self,
        ui: &mut egui::Ui,
        subscription: &SubscriptionSummary,
        show_ping: bool,
    ) {
        catalog_menu_button(ui, self.tr(Key::SubscriptionActions), |ui| {
            self.subscription_menu(ui, subscription)
        });
        if show_ping {
            self.ping_icon(ui, Key::PingTest, Some(&subscription.subscription_uuid));
        }
        if self.refresh_button(ui, subscription).clicked() {
            self.emit(Intent::RefreshSubscription(
                subscription.subscription_uuid.clone(),
            ));
        }
    }

    fn subscription_menu(&mut self, ui: &mut egui::Ui, subscription: &SubscriptionSummary) {
        let uuid = &subscription.subscription_uuid;
        let change_blocked = self.subscription_change_blocked(uuid);
        if ui.button(self.tr(Key::Routing)).clicked() {
            self.view.open_subscription_routing(uuid);
            ui.close();
        }
        if self
            .subscription_operation(ui, Key::Refresh, change_blocked)
            .clicked()
        {
            self.emit(Intent::RefreshSubscription(uuid.clone()));
            ui.close();
        }
        self.ping_menu_item(ui, Key::PingTest, Some(uuid));
        self.subscription_pin_action(ui, uuid);
        if ui.button(self.tr(Key::CopySubscriptionUrl)).clicked() {
            ui.ctx().copy_text(subscription.url.expose().to_owned());
            ui.close();
        }
        if self
            .subscription_operation(ui, Key::Remove, change_blocked)
            .clicked()
        {
            self.view.removal = Some(Removal::Subscription(uuid.clone()));
            ui.close();
        }
        ui.separator();
        if ui.button(self.tr(Key::SubscriptionSettings)).clicked() {
            self.view.open_provider_settings(uuid);
            ui.close();
        }
    }

    fn subscription_pin_action(&mut self, ui: &mut egui::Ui, uuid: &str) {
        let pinned = self.subscription_pinned(uuid);
        let key = if pinned {
            Key::UnpinSubscription
        } else {
            Key::PinSubscription
        };
        if ui.button(self.tr(key)).clicked() {
            self.view
                .pinned_subscriptions
                .insert(uuid.to_owned(), !pinned);
            ui.close();
        }
    }

    pub(super) fn subscription_change_blocked(&self, subscription_uuid: &str) -> Option<Key> {
        if self.snapshot.busy || self.view.subscription_refresh.is_some() {
            return Some(Key::Working);
        }
        self.snapshot
            .active_profile_uuid
            .as_ref()
            .and_then(|active| {
                self.snapshot
                    .profiles
                    .iter()
                    .find(|profile| &profile.profile_uuid == active)
            })
            .filter(|active| active.subscription_uuid.as_deref() == Some(subscription_uuid))
            .map(|_| Key::SubscriptionDisconnectFirst)
    }

    fn subscription_operation(
        &self,
        ui: &mut egui::Ui,
        key: Key,
        blocked: Option<Key>,
    ) -> egui::Response {
        let response = ui.add_enabled(blocked.is_none(), egui::Button::new(self.tr(key)));
        if let Some(reason) = blocked {
            return response.on_disabled_hover_text(self.tr(reason));
        }
        response
    }

    fn provider_banner(&self, ui: &mut egui::Ui, uuid: &str, display: &ProviderDisplay) {
        if let Some(notice) = &display.expiry {
            let caption = self
                .snapshot
                .subscription_usage
                .get(uuid)
                .and_then(|usage| usage.expires_at_unix)
                .and_then(|expires| self.expiry_caption(expires));
            self.provider_notice(ui, notice, caption.as_deref().unwrap_or_default());
        } else if let Some(notice) = &display.info {
            self.provider_notice(ui, notice, "");
        }
    }

    fn provider_announcement(&self, ui: &mut egui::Ui, display: &ProviderDisplay) {
        if let Some(text) = &display.announcement {
            ui.vertical_centered(|ui| {
                ui.add(egui::Label::new(text).wrap().halign(egui::Align::Center));
            });
        }
    }

    fn provider_notice(&self, ui: &mut egui::Ui, notice: &ProviderNotice, fallback_text: &str) {
        let color = match notice.color {
            ProviderNoticeColor::Red => ui.visuals().error_fg_color,
            ProviderNoticeColor::Blue => ui.visuals().hyperlink_color,
            ProviderNoticeColor::Green => crate::view::theme::success_color(ui.visuals()),
        };
        let text = if notice.text.is_empty() {
            fallback_text
        } else {
            &notice.text
        };
        if text.is_empty() && notice.button.is_none() {
            return;
        }
        egui::Frame::new()
            .fill(ui.visuals().panel_fill.lerp_to_gamma(color, 0.10))
            .corner_radius(5)
            .inner_margin(egui::Margin::same(6))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.vertical_centered(|ui| {
                    if !text.is_empty() {
                        ui.add(
                            egui::Label::new(egui::RichText::new(text).color(color))
                                .wrap()
                                .halign(egui::Align::Center),
                        );
                    }
                    if let Some(button) = &notice.button {
                        let label = if button.label.is_empty() {
                            self.tr(Key::CatalogProviderRenew)
                        } else {
                            &button.label
                        };
                        if ui
                            .add_sized(
                                [ui.available_width(), 26.0],
                                egui::Button::new(label).wrap(),
                            )
                            .clicked()
                        {
                            ui.ctx().open_url(egui::OpenUrl::new_tab(&button.url));
                        }
                    }
                });
            });
    }

    fn subscription_usage(&self, ui: &mut egui::Ui, uuid: &str, display: Option<&ProviderDisplay>) {
        let usage = self.snapshot.subscription_usage.get(uuid);
        let traffic = usage.and_then(|usage| self.traffic_caption(usage));
        let expiry = usage
            .filter(|_| display.is_none_or(|display| display.expiry.is_none()))
            .and_then(|usage| usage.expires_at_unix)
            .and_then(|expires| self.expiry_caption(expires));
        let label = traffic.as_deref().or(expiry.as_deref());
        let website = display.and_then(|display| display.website_url.as_deref());
        let support = display.and_then(|display| display.support_url.as_deref());
        if label.is_none() && website.is_none() && support.is_none() {
            return;
        }
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = TOOL_GAP;
            if let Some(url) = website {
                self.provider_link_button(ui, Key::CatalogUsageDetails, Icon::About, url);
            }
            let reserved = support.map_or(0.0, |_| TOOL_SIZE + ui.spacing().item_spacing.x);
            let width = (ui.available_width() - reserved).max(0.0);
            if let Some(label) = label {
                traffic_bar(ui, width, usage.map_or(0.0, traffic_fraction), label);
            } else {
                ui.add_space(width);
            }
            if let Some(url) = support {
                self.provider_link_button(ui, Key::CatalogProviderSupport, Icon::Link, url);
            }
        });
        if traffic.is_some()
            && let Some(expiry) = expiry
        {
            ui.weak(expiry);
        }
    }

    fn provider_link_button(&self, ui: &mut egui::Ui, key: Key, icon: Icon, url: &str) {
        let button = catalog_icon_button(ui, self.tr(key), true);
        icon.paint(
            ui.painter(),
            button.rect.center(),
            ui.visuals().hyperlink_color,
        );
        if button.clicked() {
            ui.ctx().open_url(egui::OpenUrl::new_tab(url));
        }
    }

    fn traffic_caption(&self, usage: &SubscriptionUsage) -> Option<String> {
        let used = usage.used_bytes.map(format_bytes);
        let total = usage.total_bytes.map(|bytes| match bytes {
            0 => "∞".to_owned(),
            bytes => format_bytes(bytes),
        });
        match (used.as_deref(), total.as_deref()) {
            (Some(used), Some(total)) => {
                Some(self.message(Key::CatalogTraffic, &[("used", used), ("total", total)]))
            }
            (Some(used), None) => Some(self.message(Key::CatalogTrafficUsed, &[("used", used)])),
            (None, Some(total)) => {
                Some(self.message(Key::CatalogTrafficLimit, &[("total", total)]))
            }
            (None, None) => None,
        }
    }

    fn expiry_caption(&self, unix: u64) -> Option<String> {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
        let remaining = unix.saturating_sub(now);
        let (key, count) = match remaining {
            0 => return Some(self.tr(Key::CatalogUsageExpired).to_owned()),
            1..86400 => (Key::CatalogUsageExpiresHours, remaining.div_ceil(3600)),
            _ => (Key::CatalogUsageExpiresDays, remaining.div_ceil(86400)),
        };
        Some(self.message(key, &[("count", &count.to_string())]))
    }

    fn subscription_caption(
        &self,
        provider: Option<&ProviderSubscriptionSummary>,
    ) -> Option<String> {
        let updated = subscription_updated_at(provider);
        let settings = &self.view.desktop_settings.subscriptions;
        let interval = settings.auto_update.then(|| {
            self.message(
                Key::CatalogAutoUpdateInterval,
                &[("count", &settings.interval_hours.to_string())],
            )
        });
        match (updated, interval) {
            (Some(updated), Some(interval)) => Some(format!("{updated} | {interval}")),
            (updated, interval) => updated.or(interval),
        }
    }

    fn refresh_button(
        &self,
        ui: &mut egui::Ui,
        subscription: &SubscriptionSummary,
    ) -> egui::Response {
        let uuid = subscription.subscription_uuid.as_str();
        let blocked = self.subscription_change_blocked(uuid);
        let refreshed_until = self
            .view
            .refreshed_subscription
            .as_ref()
            .filter(|(refreshed, _)| refreshed == uuid)
            .map(|(_, deadline)| *deadline);
        let refreshing = self.refreshing_subscription() == Some(uuid);
        let label = if refreshing {
            self.tr(Key::Working).into()
        } else if let Some(error) = subscription.last_error {
            viewmodel::ErrorNotice::Client(error).text(self.view.locale)
        } else if refreshed_until.is_some() {
            self.tr(Key::EventSubscriptionUpdated).into()
        } else {
            self.tr(Key::Refresh).into()
        };
        let button = catalog_icon_button(ui, &label, blocked.is_none());
        let center = button.rect.center();
        if refreshing {
            paint_spinner(ui, center, TOOL_SPINNER_SIZE);
        } else if subscription.last_error.is_some() {
            paint_failure_badge(
                ui.painter(),
                center,
                theme::ping_failure_color(ui.visuals()),
                ui.visuals().extreme_bg_color,
            );
        } else if let Some(deadline) = refreshed_until {
            let stroke = egui::Stroke::new(2.0, theme::success_color(ui.visuals()));
            ui.painter().add(egui::Shape::line(
                vec![
                    center + egui::vec2(-5.0, 0.0),
                    center + egui::vec2(-1.5, 3.5),
                    center + egui::vec2(5.5, -4.0),
                ],
                stroke,
            ));
            ui.ctx()
                .request_repaint_after(deadline.saturating_duration_since(Instant::now()));
        } else {
            paint_refresh_icon(ui, center, ui.style().interact(&button).fg_stroke);
        }
        button.on_disabled_hover_text(self.tr(blocked.unwrap_or(Key::Refresh)))
    }

    pub(in crate::view::widgets) fn refreshing_subscription(&self) -> Option<&str> {
        self.view
            .subscription_refresh
            .as_ref()
            .map(|(uuid, _)| uuid.as_str())
    }

    fn ping_blocked(&self, subscription_uuid: Option<&str>) -> Option<Key> {
        if self.snapshot.probe_batches.iter().any(|batch| {
            batch.completed < batch.total && batch.subscription_uuid.as_deref() == subscription_uuid
        }) {
            return Some(Key::Working);
        }
        if let Some(reason) = self.probe_blocked() {
            return Some(reason);
        }
        let has_profiles = self.snapshot.profiles.iter().any(|profile| {
            subscription_uuid.is_none_or(|uuid| profile.subscription_uuid.as_deref() == Some(uuid))
        });
        let has_probe_targets =
            self.snapshot
                .unavailable_profiles
                .iter()
                .any(|(uuid, profiles)| {
                    subscription_uuid.is_none_or(|selected| selected == uuid)
                        && profiles.iter().any(|profile| profile.probe_id.is_some())
                });
        (!has_profiles && !has_probe_targets).then_some(Key::NoAvailableServers)
    }

    fn ping_menu_item(&mut self, ui: &mut egui::Ui, key: Key, subscription_uuid: Option<&str>) {
        if self
            .subscription_operation(ui, key, self.ping_blocked(subscription_uuid))
            .clicked()
        {
            self.emit(Intent::ProbeProfiles(subscription_uuid.map(str::to_owned)));
            ui.close();
        }
    }

    fn ping_icon(&mut self, ui: &mut egui::Ui, key: Key, subscription_uuid: Option<&str>) {
        let blocked = self.ping_blocked(subscription_uuid);
        let button = catalog_icon_button(ui, self.tr(key), blocked.is_none());
        let center = button.rect.center();
        let batch = self.snapshot.probe_batches.iter().find(|batch| {
            batch.completed < batch.total && batch.subscription_uuid.as_deref() == subscription_uuid
        });
        if batch.is_some() {
            paint_spinner(ui, center, TOOL_SPINNER_SIZE);
        } else {
            let stroke = ui.style().interact(&button).fg_stroke;
            for (x, height) in [(-5.0, 3.0), (0.0, 6.0), (5.0, 10.0)] {
                ui.painter().line_segment(
                    [
                        center + egui::vec2(x, 4.0),
                        center + egui::vec2(x, 4.0 - height),
                    ],
                    stroke,
                );
            }
        }
        if let Some(batch) = batch {
            button.on_disabled_hover_text(format!(
                "{}: {}/{}",
                self.tr(Key::PingRunning),
                batch.completed,
                batch.total
            ));
        } else if let Some(reason) = blocked {
            button.on_disabled_hover_text(self.tr(reason));
        } else if button.clicked() {
            self.emit(Intent::ProbeProfiles(subscription_uuid.map(str::to_owned)));
        }
    }
}

fn paint_pin(painter: &egui::Painter, center: egui::Pos2, stroke: egui::Stroke) {
    let outline = [
        (-4.0, -1.5),
        (-1.5, 1.0),
        (1.5, 4.0),
        (2.5, 2.0),
        (2.0, 0.0),
        (4.5, -2.5),
        (2.5, -4.5),
        (0.0, -2.0),
        (-2.0, -2.5),
        (-4.0, -1.5),
    ];
    painter.add(egui::Shape::line(
        outline
            .into_iter()
            .map(|(x, y)| center + egui::vec2(x, y))
            .collect(),
        stroke,
    ));
    painter.line_segment(
        [
            center + egui::vec2(-1.5, 1.0),
            center + egui::vec2(-5.5, 5.0),
        ],
        stroke,
    );
}

fn subscription_updated_at(provider: Option<&ProviderSubscriptionSummary>) -> Option<String> {
    provider
        .and_then(|provider| provider.last_success_unix)
        .and_then(|unix| DateTime::from_timestamp(i64::try_from(unix).ok()?, 0))
        .map(|date| {
            date.with_timezone(&Local)
                .format("%d.%m.%Y %H:%M")
                .to_string()
        })
}

fn paint_refresh_icon(ui: &egui::Ui, center: egui::Pos2, stroke: egui::Stroke) {
    let arc = (0..=16)
        .map(|step| {
            let angle = (step as f32 * 300.0 / 16.0 + 25.0).to_radians();
            center + egui::vec2(angle.cos(), angle.sin()) * 5.0
        })
        .collect();
    ui.painter().add(egui::Shape::line(arc, stroke));
    ui.painter().add(egui::Shape::line(
        [(0.0, -2.0), (5.0, -2.0), (5.0, -7.0)]
            .map(|(x, y)| center + egui::vec2(x, y))
            .to_vec(),
        stroke,
    ));
}

fn traffic_fraction(usage: &SubscriptionUsage) -> f32 {
    usage
        .used_bytes
        .zip(usage.total_bytes.filter(|&total| total > 0))
        .map_or(0.0, |(used, total)| {
            (used.min(total) as f64 / total as f64) as f32
        })
}

fn traffic_bar(ui: &mut egui::Ui, width: f32, fraction: f32, label: &str) {
    let visuals = ui.visuals();
    let empty = visuals.extreme_bg_color;
    let fill = if fraction > 0.0 {
        empty.lerp_to_gamma(visuals.hyperlink_color, 0.4)
    } else {
        empty
    };
    let stroke = visuals.widgets.noninteractive.bg_stroke;
    let bar = ui.add(
        egui::ProgressBar::new(fraction)
            .desired_width(width)
            .desired_height(17.0)
            .corner_radius(4)
            .fill(fill),
    );
    ui.painter()
        .rect_stroke(bar.rect, 4, stroke, egui::StrokeKind::Inside);
    ui.put(
        bar.rect,
        egui::Label::new(egui::RichText::new(label).small().strong())
            .truncate()
            .selectable(false)
            .halign(egui::Align::Center),
    )
    .on_hover_text(label);
}

fn format_bytes(bytes: u64) -> String {
    for (power, unit) in [(4, "TB"), (3, "GB"), (2, "MB"), (1, "KB")] {
        let divisor = 1000_u64.pow(power);
        if bytes >= divisor {
            let size = bytes as f64 / divisor as f64;
            let rounded = format!("{size:.1}");
            return format!("{} {unit}", rounded.trim_end_matches(".0"));
        }
    }
    format!("{bytes} B")
}

fn catalog_group_frame(ui: &egui::Ui) -> egui::Frame {
    egui::Frame::new()
        .fill(ui.visuals().faint_bg_color)
        .corner_radius(6)
        .inner_margin(6)
        .outer_margin(egui::Margin {
            bottom: 6,
            ..egui::Margin::ZERO
        })
}

fn catalog_menu_button(ui: &mut egui::Ui, label: &str, contents: impl FnOnce(&mut egui::Ui)) {
    let (response, _) =
        egui::containers::menu::MenuButton::from_button(more_button(TOOL_SIZE)).ui(ui, contents);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
    response.on_hover_text(label);
}

fn catalog_icon_button(ui: &mut egui::Ui, label: &str, enabled: bool) -> egui::Response {
    let button = ui.add_enabled(
        enabled,
        egui::Button::new("")
            .frame_when_inactive(false)
            .min_size(egui::Vec2::splat(TOOL_SIZE)),
    );
    button.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, label));
    button.on_hover_text(label)
}

fn paint_chevron(ui: &egui::Ui, center: egui::Pos2, closed: bool) {
    let points = if closed {
        [(-2.0, -4.0), (2.0, 0.0), (-2.0, 4.0)]
    } else {
        [(-4.0, -2.0), (0.0, 2.0), (4.0, -2.0)]
    };
    let points = points
        .into_iter()
        .map(|(x, y)| center + egui::vec2(x, y))
        .collect();
    ui.painter().add(egui::Shape::line(
        points,
        egui::Stroke::new(1.5, ui.visuals().text_color()),
    ));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::view_model::ViewState;
    use vpn_presentation::session::{ClientSnapshot, ProfileUnavailableReason, RequestId};

    #[test]
    fn subscription_hides_empty_manual_catalog() {
        let unavailable = UnavailableProfile::new(
            "Unavailable test server",
            Some("unsupported"),
            ProfileUnavailableReason::Unsupported,
        );
        for profiles in [Vec::new(), vec![unavailable]] {
            let uuid = String::from("subscription");
            let title = "Test subscription";
            let snapshot = ClientSnapshot {
                busy: false,
                subscriptions: vec![SubscriptionSummary {
                    subscription_uuid: uuid.clone(),
                    title: title.into(),
                    auto_title: false,
                    url: vpn_presentation::session::Secret::new("https://provider.example/sub"),
                    profile_count: 0,
                    last_error: None,
                }],
                unavailable_profiles: HashMap::from([(uuid, profiles)]),
                ..ClientSnapshot::default()
            };
            let mut state = ViewState::default();
            let mut view = ViewFrame::new(&mut state, &snapshot, false, None);
            let context = egui::Context::default();
            let mut output = context.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(500.0, 400.0),
                    )),
                    ..Default::default()
                },
                |ui| view.catalog_contents(ui),
            );
            output.textures_delta.clear();
            let labels: Vec<_> = output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) => Some(text.galley.job.text.as_str()),
                    _ => None,
                })
                .collect();
            assert!(labels.contains(&title));
            for key in [Key::ServerList, Key::NoProfiles, Key::AddConfiguration] {
                assert!(!labels.contains(&view.tr(key)));
            }
        }
    }

    #[test]
    fn subscription_refresh_keeps_catalog_layout() {
        let subscription = SubscriptionSummary {
            subscription_uuid: "subscription".into(),
            title: "Test subscription".into(),
            auto_title: false,
            url: vpn_presentation::session::Secret::new("https://provider.example/sub"),
            profile_count: 0,
            last_error: None,
        };
        let mut snapshot = ClientSnapshot {
            busy: false,
            subscriptions: vec![subscription.clone()],
            ..ClientSnapshot::default()
        };

        for width in [170.0, 480.0] {
            let context = egui::Context::default();
            let mut state = ViewState::default();
            let render = |state: &mut ViewState, snapshot: &ClientSnapshot| {
                let mut view = ViewFrame::new(state, snapshot, false, None);
                let mut refresh_rect = egui::Rect::NOTHING;
                let mut output = context.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(width, 400.0),
                        )),
                        ..Default::default()
                    },
                    |ui| {
                        view.home_catalog(ui);
                        refresh_rect = view.refresh_button(ui, &subscription).rect;
                    },
                );
                output.textures_delta.clear();
                let title = output
                    .shapes
                    .iter()
                    .find_map(|shape| match &shape.shape {
                        egui::Shape::Text(text) if text.galley.job.text == subscription.title => {
                            Some((text.visual_bounding_rect(), shape.clip_rect))
                        }
                        _ => None,
                    })
                    .expect("subscription title in the catalog viewport");
                (title, refresh_rect)
            };

            let idle = render(&mut state, &snapshot);
            let request = RequestId(1);
            state.subscription_refresh = Some((subscription.subscription_uuid.clone(), request));
            snapshot.busy = true;
            assert_eq!(render(&mut state, &snapshot), idle);
            snapshot.busy = false;
            snapshot.completed_request = Some(request);
            assert_eq!(
                state.update_catalog_request(&snapshot, Instant::now()),
                None
            );
            assert_eq!(render(&mut state, &snapshot), idle);
        }
    }

    #[test]
    fn probe_keeps_subscription_controls_available() {
        let mut state = ViewState::default();
        let snapshot = ClientSnapshot {
            busy: false,
            profile_probe_available: true,
            ..Default::default()
        };
        let view = ViewFrame::new(&mut state, &snapshot, false, None);
        assert_eq!(view.subscription_change_blocked("subscription"), None);
        assert_eq!(view.probe_blocked(), None);
    }

    #[test]
    fn busy_catalog_keeps_empty_state_layout_in_both_languages() {
        for locale in [Locale::Russian, Locale::English] {
            let render = |busy| {
                let context = egui::Context::default();
                let snapshot = ClientSnapshot {
                    busy,
                    ..ClientSnapshot::default()
                };
                let mut state = ViewState::default();
                state.locale = locale;
                let mut view = ViewFrame::new(&mut state, &snapshot, false, None);
                let mut output = context.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(480.0, 400.0),
                        )),
                        ..Default::default()
                    },
                    |ui| view.home_catalog(ui),
                );
                output.textures_delta.clear();
                [Key::NoProfiles, Key::AddConfiguration].map(|key| {
                    let label = view.tr(key);
                    output
                        .shapes
                        .iter()
                        .find_map(|shape| match &shape.shape {
                            egui::Shape::Text(text) if text.galley.job.text == label => {
                                Some(text.visual_bounding_rect())
                            }
                            _ => None,
                        })
                        .expect(label)
                })
            };
            assert_eq!(render(false), render(true));
        }
    }

    #[test]
    fn subscription_menu_copies_the_saved_url_in_the_regular_client() {
        let url = "https://provider.example/sub?key=fixture";
        let subscription = SubscriptionSummary {
            subscription_uuid: "subscription".into(),
            title: "Test subscription".into(),
            auto_title: false,
            url: vpn_presentation::session::Secret::new(url),
            profile_count: 0,
            last_error: None,
        };
        let context = egui::Context::default();
        let snapshot = ClientSnapshot::default();
        let mut state = ViewState::default();
        let mut view = ViewFrame::new(&mut state, &snapshot, false, None);
        let label = view.tr(Key::CopySubscriptionUrl);
        let mut render = |events| {
            let mut output = context.run_ui(
                egui::RawInput {
                    events,
                    ..Default::default()
                },
                |ui| view.subscription_menu(ui, &subscription),
            );
            output.textures_delta.clear();
            output
        };
        let first = render(Vec::new());
        let pos = first
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.job.text == label => {
                    Some(text.visual_bounding_rect().center())
                }
                _ => None,
            })
            .expect("copy subscription URL button");
        let click = |pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        render(vec![egui::Event::PointerMoved(pos), click(true)]);
        let output = render(vec![click(false)]);
        assert!(output.platform_output.commands.iter().any(|command| {
            matches!(command, egui::OutputCommand::CopyText(text) if text == url)
        }));
    }

    #[test]
    fn empty_catalog_button_opens_configuration_import() {
        let context = egui::Context::default();
        let snapshot = ClientSnapshot {
            busy: false,
            ..ClientSnapshot::default()
        };
        let mut state = ViewState::default();
        let mut view = ViewFrame::new(&mut state, &snapshot, false, None);
        let button_label = view.tr(Key::AddConfiguration);
        let mut render = |events| {
            let mut output = context.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(500.0, 400.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| view.catalog_contents(ui),
            );
            output.textures_delta.clear();
            output.shapes
        };
        let shapes = render(Vec::new());
        let pos = shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.job.text == button_label => {
                    Some(text.visual_bounding_rect().center())
                }
                _ => None,
            })
            .expect("add configuration button");
        render(vec![egui::Event::PointerMoved(pos)]);
        for pressed in [true, false] {
            render(vec![egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            }]);
        }
        assert_eq!(
            state.configuration.map(|draft| draft.kind),
            Some(ConfigurationKind::Configuration)
        );
    }
}
