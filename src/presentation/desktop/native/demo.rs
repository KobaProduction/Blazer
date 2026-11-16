//! Local, in-memory session for checking the desktop screens.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use eframe::egui;
use vpn_presentation::Controller;
use vpn_presentation::localization::{self, Key, Locale};
use vpn_presentation::session::{
    ClientCommand, ClientError, ClientEvent, ClientEventKind, ClientSession, ClientSnapshot,
    ConnectionMode, ConnectionStatus, ProfileLatency, ProfileSummary, ProviderSubscriptionSummary,
    RequestId, SubmitError, SubscriptionSummary,
};

use super::{
    capture::{sample_catalog, sample_log_events},
    native_options,
};
use vpn_client::DesktopWindow;

pub fn run_demo() -> eframe::Result {
    eframe::run_native(
        localization::text(Locale::Russian, Key::DemoAppName),
        native_options(),
        Box::new(|creation| {
            let events = sample_log_events();
            let snapshot = ClientSnapshot {
                diagnostics: vpn_presentation::session::DiagnosticSummary {
                    buffered: events.len(),
                    dropped: 0,
                },
                diagnostic_events: events,
                ..sample_catalog()
            };
            let mut window = sample_window(&creation.egui_ctx, snapshot);
            window.enable_demo();
            Ok(Box::new(window))
        }),
    )
}

fn sample_window(context: &egui::Context, snapshot: ClientSnapshot) -> DesktopWindow {
    let dirty = Arc::new(AtomicBool::new(false));
    let session = DemoSession {
        snapshot: Arc::new(Mutex::new(snapshot)),
        dirty: Arc::clone(&dirty),
        context: context.clone(),
    };
    DesktopWindow::new(context, Controller::new(session, dirty))
}

struct DemoSession {
    snapshot: Arc<Mutex<ClientSnapshot>>,
    dirty: Arc<AtomicBool>,
    context: egui::Context,
}

impl ClientSession for DemoSession {
    fn snapshot(&self) -> ClientSnapshot {
        self.snapshot.lock().expect("demo snapshot").clone()
    }

    fn send(&self, command: ClientCommand) -> Result<RequestId, SubmitError> {
        let mut snapshot = self.snapshot.lock().map_err(|_| SubmitError::Stopped)?;
        if snapshot.busy {
            return Err(SubmitError::Busy);
        }
        let request = RequestId(snapshot.revision + 1);
        if matches!(
            command,
            ClientCommand::RefreshSubscription { .. }
                | ClientCommand::ProbeProfile { .. }
                | ClientCommand::ProbeProfiles { .. }
        ) {
            let state = Arc::clone(&self.snapshot);
            let dirty = Arc::clone(&self.dirty);
            let context = self.context.clone();
            std::thread::Builder::new()
                .name("desktop-demo-action".to_owned())
                .spawn(move || {
                    std::thread::sleep(Duration::from_millis(850));
                    if let Ok(mut state) = state.lock() {
                        complete(&mut state, request, command);
                        state.busy = false;
                    }
                    dirty.store(true, Ordering::Release);
                    context.request_repaint();
                })
                .map_err(|_| SubmitError::Stopped)?;
            snapshot.busy = true;
            snapshot.last_error = None;
        } else {
            complete(&mut snapshot, request, command);
        }
        snapshot.revision = request.0;
        drop(snapshot);
        self.dirty.store(true, Ordering::Release);
        self.context.request_repaint();
        Ok(request)
    }

    fn shutdown(&mut self) -> Result<(), ClientError> {
        Ok(())
    }
}

fn complete(snapshot: &mut ClientSnapshot, request: RequestId, command: ClientCommand) {
    let event = match &command {
        ClientCommand::SelectProfile { .. } => Some(ClientEventKind::ProfileSelected),
        ClientCommand::Connect { mode } => Some(ClientEventKind::Connected(*mode)),
        ClientCommand::Disconnect => Some(ClientEventKind::Disconnected),
        ClientCommand::RemoveProfile { .. } => Some(ClientEventKind::ProfileRemoved),
        ClientCommand::RemoveSubscription { .. } => Some(ClientEventKind::SubscriptionRemoved),
        ClientCommand::ImportText { .. } => Some(ClientEventKind::ProfilesImported),
        ClientCommand::AddSubscription { .. }
        | ClientCommand::UpdateSubscription { .. }
        | ClientCommand::RefreshSubscription { .. } => Some(ClientEventKind::SubscriptionUpdated),
        ClientCommand::RenameSubscription { .. } => {
            Some(ClientEventKind::SubscriptionSettingsSaved)
        }
        ClientCommand::SetLanguage { .. }
        | ClientCommand::SetLocalProxyPort { .. }
        | ClientCommand::SetSubscriptionHwid { .. }
        | ClientCommand::SetConnectionMode { .. } => Some(ClientEventKind::SettingsSaved),
        _ => None,
    };
    let error = apply(snapshot, request, command).err();
    snapshot.last_error = error;
    snapshot.completed_request = Some(request);
    if let Some(kind) = error.map(ClientEventKind::Failed).or(event) {
        let sequence = snapshot
            .diagnostic_events
            .last()
            .map_or(1, |event| event.sequence + 1);
        snapshot
            .diagnostic_events
            .push(ClientEvent { sequence, kind });
        snapshot.diagnostics.buffered = snapshot.diagnostic_events.len();
    }
}

fn apply(
    snapshot: &mut ClientSnapshot,
    request: RequestId,
    command: ClientCommand,
) -> Result<(), ClientError> {
    match command {
        ClientCommand::SetLanguage { language } => snapshot.language = language,
        ClientCommand::SetLocalProxyPort { port } => snapshot.local_proxy_port = port,
        ClientCommand::SetSubscriptionHwid { enabled } => snapshot.send_subscription_hwid = enabled,
        ClientCommand::SetConnectionMode { mode } => snapshot.selected_mode = mode,
        ClientCommand::SelectProfile { profile_uuid } => {
            check_profile(snapshot, &profile_uuid)?;
            snapshot.selected_profile_uuid = Some(profile_uuid);
        }
        ClientCommand::Connect { mode } => connect(snapshot, mode)?,
        ClientCommand::Disconnect => {
            snapshot.connection = ConnectionStatus::Disconnected;
            snapshot.active_profile_uuid = None;
            snapshot.active_mode = None;
        }
        ClientCommand::ProbeProfile { profile_uuid } => {
            check_profile(snapshot, &profile_uuid)?;
            let latency = demo_latency(&profile_uuid);
            snapshot.profile_latencies.insert(profile_uuid, latency);
        }
        ClientCommand::ProbeProfiles { subscription_uuid } => {
            let profiles = snapshot.profiles.iter().filter(|profile| {
                subscription_uuid
                    .as_ref()
                    .is_none_or(|uuid| profile.subscription_uuid.as_ref() == Some(uuid))
            });
            snapshot.profile_latencies.extend(profiles.map(|profile| {
                (
                    profile.profile_uuid.clone(),
                    demo_latency(&profile.profile_uuid),
                )
            }));
        }
        ClientCommand::ImportText { title, text } => {
            let protocol = import_protocol(&text)?;
            insert_profile(snapshot, request, title, protocol, None);
        }
        ClientCommand::AddSubscription { title, .. } => add_subscription(snapshot, request, title),
        ClientCommand::RefreshSubscription { subscription_uuid } => {
            refresh_subscription(snapshot, subscription_uuid)?;
        }
        ClientCommand::RemoveProfile { profile_uuid } => {
            check_profile(snapshot, &profile_uuid)?;
            snapshot
                .profiles
                .retain(|profile| profile.profile_uuid != profile_uuid);
            keep_valid_selection(snapshot);
        }
        ClientCommand::RemoveSubscription { subscription_uuid } => {
            snapshot
                .subscriptions
                .retain(|row| row.subscription_uuid != subscription_uuid);
            snapshot
                .provider_subscriptions
                .retain(|row| row.subscription_uuid != subscription_uuid);
            snapshot.subscription_usage.remove(&subscription_uuid);
            snapshot
                .profiles
                .retain(|profile| profile.subscription_uuid.as_ref() != Some(&subscription_uuid));
            keep_valid_selection(snapshot);
        }
        ClientCommand::RenameSubscription {
            subscription_uuid,
            title,
        }
        | ClientCommand::UpdateSubscription {
            subscription_uuid,
            title,
            ..
        } => {
            let locale = vpn_presentation::model::locale(snapshot.language);
            let (title, auto_title) = demo_subscription_title(locale, title);
            let subscription = snapshot
                .subscriptions
                .iter_mut()
                .find(|row| row.subscription_uuid == subscription_uuid)
                .ok_or(ClientError::SubscriptionNotFound)?;
            subscription.title = title;
            subscription.auto_title = auto_title;
        }
        _ => return Err(ClientError::UnsupportedMode),
    }
    Ok(())
}

fn demo_latency(uuid: &str) -> ProfileLatency {
    if uuid == "preview-3" {
        return ProfileLatency::TimedOut;
    }
    let milliseconds = match uuid {
        "preview-2" => 224,
        "preview-4" => 368,
        _ => 36 + uuid.bytes().map(u64::from).sum::<u64>() % 100,
    };
    ProfileLatency::Reachable { milliseconds }
}

fn refresh_subscription(snapshot: &mut ClientSnapshot, uuid: String) -> Result<(), ClientError> {
    let row = snapshot
        .subscriptions
        .iter_mut()
        .find(|row| row.subscription_uuid == uuid)
        .ok_or(ClientError::SubscriptionNotFound)?;
    row.last_error = None;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|time| time.as_secs());
    if let Some(provider) = snapshot
        .provider_subscriptions
        .iter_mut()
        .find(|row| row.subscription_uuid == uuid)
    {
        provider.last_success_unix = now;
    } else {
        snapshot
            .provider_subscriptions
            .push(ProviderSubscriptionSummary {
                subscription_uuid: uuid,
                raw_config_hidden: false,
                has_committed_fallback: false,
                last_success_unix: now,
                last_update_used_fallback: false,
                pending: Vec::new(),
                recognized_keys: Vec::new(),
                display: Default::default(),
            });
    }
    Ok(())
}

fn keep_valid_selection(snapshot: &mut ClientSnapshot) {
    snapshot.profile_latencies.retain(|uuid, _| {
        snapshot
            .profiles
            .iter()
            .any(|profile| &profile.profile_uuid == uuid)
    });
    if snapshot.selected_profile_uuid.as_ref().is_some_and(|uuid| {
        !snapshot
            .profiles
            .iter()
            .any(|profile| &profile.profile_uuid == uuid)
    }) {
        snapshot.selected_profile_uuid = snapshot
            .profiles
            .first()
            .map(|profile| profile.profile_uuid.clone());
    }
    for subscription in &mut snapshot.subscriptions {
        subscription.profile_count = snapshot
            .profiles
            .iter()
            .filter(|profile| {
                profile.subscription_uuid.as_ref() == Some(&subscription.subscription_uuid)
            })
            .count();
    }
}

fn check_profile(snapshot: &ClientSnapshot, uuid: &str) -> Result<(), ClientError> {
    if snapshot
        .profiles
        .iter()
        .any(|profile| profile.profile_uuid == uuid)
    {
        Ok(())
    } else {
        Err(ClientError::ProfileNotFound)
    }
}

fn connect(snapshot: &mut ClientSnapshot, mode: ConnectionMode) -> Result<(), ClientError> {
    if !snapshot.mode_capabilities.supports(mode) {
        return Err(ClientError::UnsupportedMode);
    }
    let selected = snapshot
        .selected_profile_uuid
        .as_deref()
        .ok_or(ClientError::ProfileNotFound)?;
    check_profile(snapshot, selected)?;
    snapshot.active_profile_uuid = Some(selected.to_owned());
    snapshot.active_mode = Some(mode);
    snapshot.connection = ConnectionStatus::Connected;
    Ok(())
}

fn import_protocol(text: &str) -> Result<String, ClientError> {
    let text = text.trim();
    if text.starts_with('{') || text.starts_with('[') {
        return Ok("JSON".to_owned());
    }
    let (scheme, _) = text
        .split_once("://")
        .ok_or(ClientError::UnsupportedFormat)?;
    if scheme.is_empty()
        || scheme.len() > 16
        || !scheme.bytes().all(|byte| byte.is_ascii_alphanumeric())
    {
        return Err(ClientError::UnsupportedFormat);
    }
    Ok(scheme.to_ascii_uppercase())
}

fn insert_profile(
    snapshot: &mut ClientSnapshot,
    request: RequestId,
    name: String,
    protocol: String,
    subscription_uuid: Option<String>,
) {
    let profile_uuid = format!("demo-profile-{}", request.0);
    snapshot.profiles.push(ProfileSummary {
        profile_uuid,
        name,
        protocol,
        server: "203.0.113.4:443".to_owned(),
        subscription_uuid,
    });
}

fn add_subscription(snapshot: &mut ClientSnapshot, request: RequestId, title: String) {
    let subscription_uuid = format!("demo-subscription-{}", request.0);
    let locale = vpn_presentation::model::locale(snapshot.language);
    let (title, auto_title) = demo_subscription_title(locale, title);
    snapshot.subscriptions.push(SubscriptionSummary {
        subscription_uuid: subscription_uuid.clone(),
        title: title.clone(),
        auto_title,
        profile_count: 1,
        last_error: None,
    });
    insert_profile(
        snapshot,
        request,
        title,
        "SOCKS5".to_owned(),
        Some(subscription_uuid),
    );
}

fn demo_subscription_title(locale: Locale, title: String) -> (String, bool) {
    if title.is_empty() {
        let fallback = vpn_presentation::import::ImportKind::Subscription.default_title(locale);
        (fallback.to_owned(), true)
    } else {
        (title, false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refresh_and_subscription_probe_publish_busy_then_update_only_the_group() {
        let mut snapshot = sample_catalog();
        snapshot.profile_latencies.insert(
            "preview-4".to_owned(),
            ProfileLatency::Reachable { milliseconds: 1 },
        );
        let session = DemoSession {
            snapshot: Arc::new(Mutex::new(snapshot)),
            dirty: Arc::default(),
            context: egui::Context::default(),
        };
        let uuid = session.snapshot().subscriptions[0]
            .subscription_uuid
            .clone();
        let previous_update = session.snapshot().provider_subscriptions[0].last_success_unix;
        let refresh = session
            .send(ClientCommand::RefreshSubscription {
                subscription_uuid: uuid.clone(),
            })
            .unwrap();
        assert!(session.snapshot().busy);
        assert_eq!(
            session.send(ClientCommand::ProbeProfiles {
                subscription_uuid: None
            }),
            Err(SubmitError::Busy)
        );
        let updated = wait_for_request(&session, refresh);
        assert!(updated.provider_subscriptions[0].last_success_unix > previous_update);
        assert_eq!(
            updated.diagnostic_events.last().unwrap().kind,
            ClientEventKind::SubscriptionUpdated
        );

        let probe = session
            .send(ClientCommand::ProbeProfiles {
                subscription_uuid: Some(uuid),
            })
            .unwrap();
        assert!(session.snapshot().busy);
        let probed = wait_for_request(&session, probe);
        assert_eq!(
            probed.profile_latencies.get("preview-1"),
            Some(&demo_latency("preview-1"))
        );
        for uuid in ["preview-4", "preview-5"] {
            assert_eq!(
                probed.profile_latencies.get(uuid),
                updated.profile_latencies.get(uuid)
            );
        }
    }

    fn wait_for_request(session: &DemoSession, request: RequestId) -> ClientSnapshot {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let snapshot = session.snapshot();
            if snapshot.completed_request == Some(request) {
                assert!(!snapshot.busy);
                assert_eq!(snapshot.last_error, None);
                return snapshot;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn demo_can_add_select_connect_and_disconnect_without_a_backend() {
        let mut snapshot = sample_catalog();
        apply(
            &mut snapshot,
            RequestId(1),
            ClientCommand::ImportText {
                title: "🇫🇮 Helsinki".to_owned(),
                text: "socks5://example.invalid:1080".to_owned(),
            },
        )
        .unwrap();
        let last = snapshot.profiles.last().unwrap();
        assert_eq!(last.name, "🇫🇮 Helsinki");
        let uuid = last.profile_uuid.clone();
        apply(
            &mut snapshot,
            RequestId(2),
            ClientCommand::SelectProfile {
                profile_uuid: uuid.clone(),
            },
        )
        .unwrap();
        apply(
            &mut snapshot,
            RequestId(3),
            ClientCommand::Connect {
                mode: ConnectionMode::LocalProxy,
            },
        )
        .unwrap();
        assert_eq!(snapshot.active_profile_uuid, Some(uuid));
        assert_eq!(snapshot.connection, ConnectionStatus::Connected);
        apply(&mut snapshot, RequestId(4), ClientCommand::Disconnect).unwrap();
        assert_eq!(snapshot.connection, ConnectionStatus::Disconnected);
        assert!(snapshot.active_profile_uuid.is_none());
    }
}
