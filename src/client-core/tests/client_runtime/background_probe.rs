use super::*;
use client_core::{ProbeHttpMethod, ProbeMethod, ProbeSettings, ProbeStrategy, ProfileProbeState};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use vpn_application::client::OwnedProbeTask;

#[derive(Default)]
struct Control {
    active: AtomicUsize,
    maximum: AtomicUsize,
    release: AtomicBool,
    ignore_cancel: bool,
    seen: Mutex<Vec<ProbeSettings>>,
}

struct Engine {
    control: Arc<Control>,
    running: Option<ConnectionMode>,
}

impl ClientEnginePort for Engine {
    fn capabilities(&self) -> ModeCapabilities {
        ModeCapabilities {
            local_proxy: true,
            tun: true,
            ..Default::default()
        }
    }
    fn validate_profile(&self, _: &ProxyProfile) -> Result<(), ClientError> {
        Ok(())
    }
    fn supports_tunnel_probe(&self) -> bool {
        true
    }
    fn supports_active_tunnel_probe(&self) -> bool {
        true
    }
    fn prepare_profile_probe(
        &self,
        profile: &ProxyProfile,
        _: Option<&ProxyProfile>,
        _: bool,
        settings: &ProbeSettings,
    ) -> Result<OwnedProbeTask, ClientError> {
        let control = self.control.clone();
        let port = profile.port;
        let fails = profile.name == "Error";
        let settings = settings.clone();
        Ok(Box::new(move |cancel| {
            let active = control.active.fetch_add(1, Ordering::SeqCst) + 1;
            control.maximum.fetch_max(active, Ordering::SeqCst);
            struct Release(Arc<Control>);
            impl Drop for Release {
                fn drop(&mut self) {
                    self.0.active.fetch_sub(1, Ordering::SeqCst);
                }
            }
            let _release = Release(control.clone());
            control.seen.lock().unwrap().push(settings);
            let deadline = Instant::now() + Duration::from_secs(4);
            while !control.release.load(Ordering::SeqCst) {
                if !control.ignore_cancel {
                    cancel.check()?;
                }
                if Instant::now() >= deadline {
                    return Err(ClientError::Timeout);
                }
                thread::sleep(Duration::from_millis(2));
            }
            if control.ignore_cancel && port == 18017 {
                thread::sleep(Duration::from_millis(120));
            }
            if fails {
                Err(ClientError::Tls)
            } else {
                Ok(Duration::from_millis(u64::from(port % 100)))
            }
        }))
    }
    fn start(
        &mut self,
        mode: ConnectionMode,
        _: &ProxyProfile,
        _: u16,
        cancel: &Cancellation,
    ) -> Result<(), ClientError> {
        cancel.check()?;
        assert_eq!(
            self.control.active.load(Ordering::SeqCst),
            0,
            "connection mutation preceded probe cleanup"
        );
        self.running = Some(mode);
        Ok(())
    }
    fn stop(&mut self) -> Result<(), ClientError> {
        self.running = None;
        Ok(())
    }
    fn poll(&mut self) -> Result<EngineRuntime, ClientError> {
        Ok(match self.running {
            Some(ConnectionMode::Tun) => EngineRuntime::Running(RunningConnection::Tun),
            Some(_) => EngineRuntime::Running(RunningConnection::LocalProxy(SocketAddr::from((
                Ipv4Addr::LOCALHOST,
                1080,
            )))),
            None => EngineRuntime::Stopped,
        })
    }
}

fn backend(directory: &TestDirectory, control: Arc<Control>, count: usize) -> TestBackend {
    let client = ClientHandle::spawn_with_platform(
        directory.path.clone(),
        Box::new(UserSecretProtection),
        Box::new(Engine {
            control,
            running: None,
        }),
        SubscriptionRefreshMode::Manual,
    )
    .unwrap();
    let backend = TestBackend::from_handle(client);
    let links = (0..count)
        .map(|i| {
            format!(
                "socks5://127.0.0.1:{}#{}",
                18017 + i,
                if i == 7 {
                    "Error".into()
                } else {
                    format!("P{i}")
                }
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    backend.run(ClientCommand::ImportText {
        title: "Fixture".into(),
        text: links,
    });
    backend.run(ClientCommand::SetProbeSettings {
        settings: ProbeSettings {
            method: ProbeMethod::Proxy,
            http_method: ProbeHttpMethod::Head,
            url: "http://health.fixture.invalid/custom".into(),
            timeout_ms: 250,
            attempts: 3,
            warmup: false,
            strategy: ProbeStrategy::Minimum,
        },
    });
    backend
}

fn completed(snapshot: &ClientSnapshot, request: RequestId) -> bool {
    snapshot
        .probe_completions
        .iter()
        .any(|item| item.request_id == request)
}

#[test]
fn profile_background_probe_limits_all_requests_to_five_without_blocking_commands() {
    let directory = TestDirectory::new();
    let control = Arc::new(Control::default());
    let mut backend = backend(&directory, control.clone(), 8);
    let batch = backend.send(ClientCommand::ProbeProfiles {
        subscription_uuid: None,
    });
    let running = backend.wait_for_schedule(|snapshot| {
        snapshot
            .profile_probes
            .values()
            .filter(|item| item.state == ProfileProbeState::Running)
            .count()
            == 5
    });
    assert!(!running.busy);
    assert_eq!(running.probe_batches[0].total, 8);
    let uuid = running.profiles[2].profile_uuid.clone();
    let single = backend.send(ClientCommand::ProbeProfile { profile_uuid: uuid });
    let ack = backend.send(ClientCommand::SetLanguage {
        language: client_core::Language::English,
    });
    let ordinary = backend.wait(Some(ack), "independent settings while probes run");
    assert_eq!(ordinary.last_error, None);
    assert_eq!(control.active.load(Ordering::SeqCst), 5);
    control.release.store(true, Ordering::SeqCst);
    let done = backend
        .wait_for_schedule(|snapshot| completed(snapshot, batch) && completed(snapshot, single));
    assert_eq!(
        done.completed_request,
        Some(ack),
        "probe results replaced an editor/settings ACK"
    );
    assert_eq!(control.maximum.load(Ordering::SeqCst), 5);
    assert_eq!(
        control.seen.lock().unwrap().len(),
        8,
        "overlapping per-profile request spawned a duplicate"
    );
    assert!(
        done.profile_probes
            .values()
            .all(|status| status.method == ProbeMethod::Proxy
                && status.state == ProfileProbeState::Finished)
    );
    let failed = done
        .profiles
        .iter()
        .find(|profile| profile.name == "Error")
        .unwrap();
    assert_eq!(
        done.profile_probes[&failed.profile_uuid].error,
        Some(ClientError::Tls)
    );
    assert_eq!(done.last_error, None);
    assert!(
        control
            .seen
            .lock()
            .unwrap()
            .iter()
            .all(|settings| settings.url.ends_with("/custom")
                && settings.http_method == ProbeHttpMethod::Head
                && settings.attempts == 3)
    );
    assert_single_tls_failure(&backend, &failed.profile_uuid);
    let invalidated = backend.run(ClientCommand::SetProbeSettings {
        settings: ProbeSettings::default(),
    });
    assert!(invalidated.profile_latencies.is_empty());
    assert!(invalidated.profile_probes.is_empty());
    backend.handle.shutdown().unwrap();
}

fn assert_single_tls_failure(backend: &TestBackend, profile_uuid: &str) {
    let request = backend.send(ClientCommand::ProbeProfile {
        profile_uuid: profile_uuid.to_owned(),
    });
    let snapshot = backend.wait(Some(request), "single probe TLS failure");
    assert_eq!(probe_error(&snapshot, request), Some(ClientError::Tls));
    assert_eq!(snapshot.last_error, None);
}

#[test]
fn profile_background_probe_keeps_cancelled_workers_counted_and_rejects_stale_revisions() {
    let directory = TestDirectory::new();
    let control = Arc::new(Control {
        ignore_cancel: true,
        ..Default::default()
    });
    let mut backend = backend(&directory, control.clone(), 5);
    let batch = backend.send(ClientCommand::ProbeProfiles {
        subscription_uuid: None,
    });
    let running = backend.wait_for_schedule(|snapshot| {
        snapshot
            .profile_probes
            .values()
            .filter(|item| item.state == ProfileProbeState::Running)
            .count()
            == 5
    });
    let uuid = running.profiles[0].profile_uuid.clone();
    backend.run(ClientCommand::UpdateProfile {
        profile_uuid: uuid.clone(),
        title: None,
        text: Secret::new("socks5://127.0.0.1:18099#Updated"),
    });
    let single = backend.send(ClientCommand::ProbeProfile {
        profile_uuid: uuid.clone(),
    });
    backend.wait_for_schedule(|snapshot| {
        snapshot.profile_probes.get(&uuid).is_some_and(|item| {
            item.request_id == single && item.state == ProfileProbeState::Queued
        })
    });
    assert_eq!(control.active.load(Ordering::SeqCst), 5);
    control.release.store(true, Ordering::SeqCst);
    backend.wait_for_schedule(|snapshot| completed(snapshot, single));
    let connected = backend.run(ClientCommand::Connect {
        mode: ConnectionMode::LocalProxy,
    });
    assert_eq!(connected.connection, ConnectionStatus::Connected);
    assert_eq!(control.active.load(Ordering::SeqCst), 0);
    assert!(completed(&connected, batch));
    assert_eq!(
        connected.profile_latencies[&uuid],
        ProfileLatency::Reachable { milliseconds: 99 }
    );
    assert_eq!(control.maximum.load(Ordering::SeqCst), 5);
    backend.run(ClientCommand::Disconnect);
    backend.handle.shutdown().unwrap();
}
