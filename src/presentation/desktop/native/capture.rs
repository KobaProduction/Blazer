use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, SyncSender};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use eframe::egui;
use vpn_client::{
    ConfigurationKind, ConfigurationPreview, DesktopWindow, Screen, SettingsPage,
    selector_capture_id,
};
use vpn_presentation::Controller;
use vpn_presentation::localization::{self, Key, Locale};
use vpn_presentation::session::{
    ClientCommand, ClientError, ClientEvent, ClientEventKind, ClientSession, ClientSnapshot,
    ConnectionMode, ConnectionStatus, ModeCapabilities, ProfileLatency, ProfileSummary,
    ProviderChangeKind, ProviderChangePreview, ProviderDisplay, ProviderNotice,
    ProviderNoticeButton, ProviderNoticeColor, ProviderSubscriptionSummary, RequestId, RouteAction,
    RouteRule, RouteTarget, RoutingAssignment, RoutingInput, RoutingProfile, SubmitError,
    SubscriptionSummary,
};

use super::native_options;

pub fn capture(path: PathBuf, screen: &OsStr) -> Result<(), Box<dyn std::error::Error>> {
    let compact = matches!(
        screen.to_str(),
        Some("home-compact" | "settings-compact" | "add-compact")
    );
    let expanded = screen == "home-expanded";
    let cidr_edit = screen == "excluded-routes-edit";
    let refresh = match screen.to_str() {
        Some("subscription-refreshing") => Some((true, None)),
        Some("subscription-refresh-success") => Some((false, None)),
        Some("subscription-refresh-error" | "subscription-notification") => {
            Some((false, Some(ClientError::Network)))
        }
        _ => None,
    };
    let selector = match screen.to_str() {
        Some("mode-picker") => Some(("connection-mode", true)),
        Some("mode-hover") => Some(("connection-mode", false)),
        _ => None,
    };
    let sample = match screen.to_str() {
        Some(
            "home-sample"
            | "home-expanded"
            | "home-compact"
            | "edit-subscription"
            | "add"
            | "add-compact"
            | "add-config"
            | "add-json"
            | "add-qr"
            | "server-editor"
            | "mode-picker"
            | "mode-hover"
            | "import-picker"
            | "subscription-notification"
            | "subscription-refreshing"
            | "subscription-refresh-success"
            | "subscription-refresh-error",
        ) => Some(sample_catalog()),
        Some("home-connected") => Some(sample_connection(ConnectionStatus::Connected)),
        Some("home-connecting") => Some(sample_connection(ConnectionStatus::Connecting)),
        Some("inbounds") => Some(sample_catalog()),
        Some("subscription-sample") => Some(sample_provider_settings()),
        Some("logs-sample") => Some(sample_logs()),
        Some("routing" | "routing-editor" | "subscription-routing") => Some(sample_routing()),
        _ => None,
    };
    let form = capture_form(screen);
    let screen = capture_screen(screen)?;
    let (sender, receiver) = mpsc::sync_channel(1);
    let mut options = native_options();
    options.viewport = options.viewport.with_active(false);
    if compact {
        options.viewport = options.viewport.with_inner_size([700.0, 520.0]);
    }
    eframe::run_native(
        localization::text(Locale::Russian, Key::DemoAppName),
        options,
        Box::new(move |creation| {
            let mut snapshot = sample.unwrap_or_else(|| ClientSnapshot {
                busy: false,
                ..Default::default()
            });
            let refresh = refresh.map(|(busy, error)| {
                let request = RequestId(1);
                snapshot.busy = busy;
                snapshot.completed_request = (!busy).then_some(request);
                snapshot.last_error = error;
                snapshot.subscriptions[0].last_error = error;
                (snapshot.subscriptions[0].subscription_uuid.clone(), request)
            });
            let session = CaptureSession(snapshot.clone());
            let mut window =
                DesktopWindow::new(&creation.egui_ctx, Controller::new(session, Arc::default()));
            creation
                .egui_ctx
                .global_style_mut(|style| style.animation_time = 0.0);
            window.preview(screen, &snapshot, form);
            window.preview_layout(expanded || compact, if compact { 200 } else { 100 });
            if let Some((uuid, request)) = refresh {
                window.preview_subscription_refresh(&uuid, request);
            }
            if cidr_edit {
                window.preview_cidr();
            }
            if let Some((target, _)) = selector {
                creation
                    .egui_ctx
                    .data_mut(|data| data.insert_temp(selector_capture_id(), target));
            }
            Ok(Box::new(CaptureApp {
                window,
                request: Some((path, sender)),
                deadline: Instant::now() + Duration::from_secs(8),
                warmup_frames: if selector.is_some() { 8 } else { 2 },
                selector_click: selector.map(|_| 0),
                open_selector: selector.is_some_and(|(_, open)| open),
            }))
        }),
    )?;
    receiver
        .try_recv()
        .map_err(|_| io::Error::other("no rendered frame was captured"))??;
    Ok(())
}

struct CaptureSession(ClientSnapshot);

impl ClientSession for CaptureSession {
    fn snapshot(&self) -> ClientSnapshot {
        self.0.clone()
    }

    fn send(&self, _command: ClientCommand) -> Result<RequestId, SubmitError> {
        Err(SubmitError::Stopped)
    }

    fn shutdown(&mut self) -> Result<(), ClientError> {
        Ok(())
    }
}

fn capture_screen(screen: &OsStr) -> Result<Screen, io::Error> {
    let screen = match screen.to_str() {
        Some(
            "home"
            | "home-sample"
            | "home-expanded"
            | "home-compact"
            | "subscriptions"
            | "home-connected"
            | "home-connecting"
            | "profiles"
            | "add"
            | "add-compact"
            | "add-config"
            | "add-json"
            | "add-qr"
            | "edit-subscription"
            | "server-editor"
            | "mode-picker"
            | "mode-hover"
            | "import-picker"
            | "subscription-notification"
            | "subscription-refreshing"
            | "subscription-refresh-success"
            | "subscription-refresh-error",
        ) => Screen::Home,
        Some("routing") => Screen::Settings(SettingsPage::Routing),
        Some("routing-profile" | "routing-editor") => {
            Screen::Settings(SettingsPage::RoutingProfile)
        }
        Some("subscription-routing") => Screen::Settings(SettingsPage::SubscriptionRouting),
        Some("logs" | "logs-sample") => Screen::Settings(SettingsPage::Logs),
        Some("statistics") => Screen::Statistics,
        Some("settings" | "settings-compact") => Screen::Settings(SettingsPage::Index),
        Some("general" | "interface") => Screen::Settings(SettingsPage::Index),
        Some("advanced" | "vpn" | "network") => Screen::Settings(SettingsPage::Advanced),
        Some("inbounds") => Screen::Settings(SettingsPage::Inbounds),
        Some("excluded-routes" | "excluded-routes-edit") => {
            Screen::Settings(SettingsPage::ExcludedRoutes)
        }
        Some("url-schemes") => Screen::Settings(SettingsPage::UrlSchemes),
        Some("per-app-proxy") => Screen::Settings(SettingsPage::PerAppProxy),
        Some("subscription-settings") => Screen::Settings(SettingsPage::Subscriptions),
        Some("subscription-sample") => Screen::Settings(SettingsPage::Subscription),
        Some("ping") => Screen::Settings(SettingsPage::Ping),
        Some("about") => Screen::Settings(SettingsPage::About),
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "unknown screenshot screen",
            ));
        }
    };
    Ok(screen)
}

fn capture_form(screen: &OsStr) -> Option<ConfigurationPreview> {
    let kind = match screen.to_str()? {
        "add" | "add-compact" | "import-picker" => ConfigurationKind::Subscription,
        "add-config" | "add-json" => ConfigurationKind::Configuration,
        "add-qr" => ConfigurationKind::Qr,
        "edit-subscription" => return Some(ConfigurationPreview::EditSubscription),
        "server-editor" => return Some(ConfigurationPreview::EditProfile),
        _ => return None,
    };
    Some(ConfigurationPreview::Add(kind))
}

pub(super) fn sample_catalog() -> ClientSnapshot {
    let source = "preview-subscription";
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let profiles = [
        ("preview-1", "🇩🇪 Франкфурт", "VLESS"),
        (
            "preview-2",
            "🇳🇱 Амстердам · резервный сервер с длинным названием",
            "Shadowsocks",
        ),
        ("preview-3", "🇫🇮 Хельсинки", "SOCKS5"),
        ("preview-4", "🇯🇵 Токио (вручную)", "VMess"),
        ("preview-5", "Сервер без страны", "SOCKS5"),
    ];
    ClientSnapshot {
        profiles: profiles
            .into_iter()
            .map(|(uuid, name, protocol)| ProfileSummary {
                profile_uuid: uuid.to_owned(),
                name: name.to_owned(),
                protocol: protocol.to_owned(),
                server: "203.0.113.1:443".to_owned(),
                subscription_uuid: (uuid != "preview-4" && uuid != "preview-5")
                    .then(|| source.to_owned()),
            })
            .collect(),
        subscriptions: vec![SubscriptionSummary {
            subscription_uuid: source.to_owned(),
            title: "Пример подписки".to_owned(),
            auto_title: false,
            profile_count: 3,
            last_error: None,
        }],
        subscription_usage: [(
            source.to_owned(),
            vpn_presentation::session::SubscriptionUsage {
                used_bytes: Some(27_000_000_000),
                total_bytes: Some(100_000_000_000),
                expires_at_unix: Some(now + 14 * 86_400),
            },
        )]
        .into(),
        provider_subscriptions: vec![ProviderSubscriptionSummary {
            subscription_uuid: source.to_owned(),
            raw_config_hidden: false,
            has_committed_fallback: false,
            last_success_unix: Some(now.saturating_sub(3 * 3600)),
            last_update_used_fallback: false,
            pending: Vec::new(),
            recognized_keys: Vec::new(),
            display: ProviderDisplay {
                info: Some(ProviderNotice {
                    text: "У подписки обновились адреса серверов".to_owned(),
                    color: ProviderNoticeColor::Blue,
                    button: Some(ProviderNoticeButton {
                        label: "Подробнее".to_owned(),
                        url: "https://support.example/news".to_owned(),
                    }),
                }),
                announcement: Some("Если сервер недоступен, выберите другой из списка.".to_owned()),
                support_url: Some("https://support.example/".to_owned()),
                ..ProviderDisplay::default()
            },
        }],
        selected_profile_uuid: Some("preview-1".to_owned()),
        profile_latencies: [
            (
                "preview-1".to_owned(),
                ProfileLatency::Reachable { milliseconds: 73 },
            ),
            (
                "preview-2".to_owned(),
                ProfileLatency::Reachable { milliseconds: 224 },
            ),
            ("preview-3".to_owned(), ProfileLatency::TimedOut),
            (
                "preview-4".to_owned(),
                ProfileLatency::Reachable { milliseconds: 368 },
            ),
        ]
        .into(),
        tcp_probe_available: true,
        mode_capabilities: ModeCapabilities {
            local_proxy: true,
            local_http_proxy: true,
            tun: true,
            ..ModeCapabilities::default()
        },
        busy: false,
        ..ClientSnapshot::default()
    }
}

fn sample_connection(status: ConnectionStatus) -> ClientSnapshot {
    let mut snapshot = sample_catalog();
    snapshot.connection = status;
    snapshot.active_profile_uuid = snapshot.selected_profile_uuid.clone();
    snapshot.active_mode = Some(ConnectionMode::LocalProxy);
    snapshot
}

fn sample_logs() -> ClientSnapshot {
    ClientSnapshot {
        diagnostic_events: sample_log_events(),
        busy: false,
        ..ClientSnapshot::default()
    }
}

pub(super) fn sample_log_events() -> Vec<ClientEvent> {
    let kinds = [
        ClientEventKind::Started,
        ClientEventKind::ProfilesImported,
        ClientEventKind::SubscriptionUpdated,
        ClientEventKind::ProfileSelected,
        ClientEventKind::SettingsSaved,
    ];
    kinds
        .into_iter()
        .enumerate()
        .map(|(index, kind)| ClientEvent {
            sequence: index as u64 + 1,
            kind,
        })
        .collect()
}

fn sample_routing() -> ClientSnapshot {
    let mut snapshot = sample_catalog();
    let profile = RoutingProfile {
        uuid: "preview-routing-profile".to_owned(),
        revision: 1,
        input: RoutingInput {
            title: "Пример правил".to_owned(),
            rules: vec![RouteRule {
                target: RouteTarget::DomainSuffix("example.org".to_owned()),
                action: RouteAction::Direct,
            }],
            ..Default::default()
        },
    };
    snapshot.routing.catalog.assignments.insert(
        snapshot.subscriptions[0].subscription_uuid.clone(),
        RoutingAssignment {
            enabled: true,
            profile_uuid: Some(profile.uuid.clone()),
            ..Default::default()
        },
    );
    snapshot.routing.saved = Some((&profile).into());
    snapshot.routing.catalog.profiles.push(profile);
    snapshot.routing.local_proxy_available = true;
    snapshot
}

fn sample_provider_settings() -> ClientSnapshot {
    let mut snapshot = sample_catalog();
    let provider = &mut snapshot.provider_subscriptions[0];
    provider.raw_config_hidden = true;
    provider.pending.push(ProviderChangePreview {
        kind: ProviderChangeKind::Migration,
        nonce: "preview-only".to_owned(),
        from_origin: "https://primary.example".to_owned(),
        target_origin: "https://new.example".to_owned(),
    });
    snapshot
}

struct CaptureApp {
    window: DesktopWindow,
    request: Option<(PathBuf, SyncSender<image::ImageResult<()>>)>,
    deadline: Instant,
    warmup_frames: usize,
    selector_click: Option<u8>,
    open_selector: bool,
}

impl eframe::App for CaptureApp {
    fn raw_input_hook(&mut self, context: &egui::Context, input: &mut egui::RawInput) {
        if self.warmup_frames > 4 {
            return;
        }
        let Some(phase) = self.selector_click else {
            return;
        };
        let Some(pos) = context
            .data(|data| data.get_temp::<egui::Pos2>(selector_capture_id().with("position")))
        else {
            return;
        };
        input.events.push(egui::Event::PointerMoved(pos));
        if phase != 0 {
            input.events.push(egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: phase == 1,
                modifiers: egui::Modifiers::NONE,
            });
        }
        self.selector_click = (self.open_selector && phase < 2).then_some(phase + 1);
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        self.window.ui(ui, frame);
        if Instant::now() >= self.deadline {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if self.warmup_frames > 0 {
            self.warmup_frames -= 1;
            ui.ctx().request_repaint();
            return;
        }
        if self.open_selector && !egui::Popup::is_any_open(ui.ctx()) {
            ui.ctx().request_repaint_after(Duration::from_millis(25));
            return;
        }
        if let Some((path, sender)) = self.request.take() {
            let context = ui.ctx().clone();
            context.request_repaint_after(self.deadline.saturating_duration_since(Instant::now()));
            ui.ctx().request_screenshot(move |image| {
                let _ = sender.send(write_png(&path, &image));
                context.send_viewport_cmd(egui::ViewportCommand::Close);
            });
        }
    }

    fn on_exit(&mut self, gl: Option<&eframe::glow::Context>) {
        self.window.on_exit(gl);
    }
}

fn write_png(path: &Path, pixels: &egui::ColorImage) -> image::ImageResult<()> {
    image::save_buffer_with_format(
        path,
        pixels.as_raw(),
        pixels.width() as u32,
        pixels.height() as u32,
        image::ColorType::Rgba8,
        image::ImageFormat::Png,
    )
}
