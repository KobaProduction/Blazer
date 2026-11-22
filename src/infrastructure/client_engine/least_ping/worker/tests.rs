use super::*;
use crate::client_engine::least_ping::{Monitor, SelectorControlFuture};
use std::sync::{Arc, Mutex};
use vpn_domain::client::{
    GraphGroup, GraphObservatory, GraphOutbound, GraphOutboundKind, GraphStrategy, GraphTarget,
    ProxyGraph, ProxyProfile, ProxyProtocol, ProxySecurity, ProxyTransport, Secret,
};

#[derive(Default)]
struct EmbeddedControl {
    calls: Mutex<Vec<(String, String)>>,
    fail: bool,
}

impl SelectorControl for EmbeddedControl {
    fn ready(&self) -> SelectorControlFuture<'_> {
        Box::pin(async move {
            if self.fail {
                Err(ClientError::UnsupportedFeature)
            } else {
                Ok(())
            }
        })
    }
    fn select<'a>(&'a self, group: &'a str, member: &'a str) -> SelectorControlFuture<'a> {
        Box::pin(async move {
            self.calls
                .lock()
                .unwrap()
                .push((group.into(), member.into()));
            Ok(())
        })
    }
}

fn profile() -> ProxyProfile {
    let provider = ProxyGraph {
        outbounds: ["a", "b", "fallback"]
            .map(|tag| GraphOutbound {
                tag: tag.into(),
                kind: GraphOutboundKind::Direct,
            })
            .into(),
        groups: vec![GraphGroup {
            tag: "least".into(),
            members: vec!["a".into(), "b".into()],
            strategy: GraphStrategy::LeastPing {
                fallback: Some("fallback".into()),
            },
        }],
        rules: Vec::new(),
        default_target: GraphTarget::Outbound("fallback".into()),
        dns: vpn_domain::routing::DnsPolicy::Automatic,
        observatory: Some(GraphObservatory {
            members: vec!["a".into(), "b".into()],
            probe_url: Secret::new("http://probe.invalid/"),
            interval: Duration::from_secs(60),
            enable_concurrency: false,
        }),
    };
    ProxyProfile {
        profile_uuid: "embedded".into(),
        name: "Embedded".into(),
        source: None,
        host: "placeholder.invalid".into(),
        port: 443,
        protocol: ProxyProtocol::Graph(Box::new(provider)),
        transport: ProxyTransport::Tcp,
        security: ProxySecurity::None,
    }
}

#[test]
fn embedded_records_do_not_require_an_http_controller_and_keep_probes_authenticated() {
    let prepared = Prepared::for_embedded_profile(&profile()).unwrap().unwrap();
    let mut records = Vec::new();
    prepared.decorate_embedded(&mut records).unwrap();
    assert_eq!(records.len(), 2);
    assert!(
        records
            .iter()
            .all(|record| record.get("clash_api_address").is_none())
    );
    for record in records {
        assert_eq!(record["protocol"]["type"], "socks");
        assert_eq!(record["protocol"]["username"], "observer");
        assert!(record["protocol"]["password"].as_str().unwrap().len() >= 32);
        assert!(
            record["address"]
                .as_str()
                .unwrap()
                .starts_with("127.0.0.1:")
        );
    }
}

#[test]
fn embedded_selector_port_receives_shared_health_selection_failure_and_recovery() {
    let prepared = Prepared::for_embedded_profile(&profile()).unwrap().unwrap();
    let control = Arc::new(EmbeddedControl::default());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let mut observer = Observer::new(prepared, control.clone()).unwrap();
        observer
            .record("a", Some(Duration::from_millis(5)))
            .await
            .unwrap();
        observer
            .record("b", Some(Duration::from_millis(150)))
            .await
            .unwrap();
        observer.record("a", None).await.unwrap();
        observer.record("b", None).await.unwrap();
        observer
            .record("a", Some(Duration::from_millis(5)))
            .await
            .unwrap();
    });
    assert_eq!(
        *control.calls.lock().unwrap(),
        [
            (
                "blazer-graph-group-0".into(),
                "blazer-graph-outbound-0".into()
            ),
            (
                "blazer-graph-group-0".into(),
                "blazer-graph-outbound-1".into()
            ),
            (
                "blazer-graph-group-0".into(),
                "blazer-graph-outbound-2".into()
            ),
            (
                "blazer-graph-group-0".into(),
                "blazer-graph-outbound-0".into()
            ),
        ]
    );
}

#[test]
fn a_missing_embedded_selector_capability_fails_before_observations_start() {
    let prepared = Prepared::for_embedded_profile(&profile()).unwrap().unwrap();
    let cancel = Cancellation::new(Arc::default(), 1);
    let control = Arc::new(EmbeddedControl {
        fail: true,
        ..Default::default()
    });
    let result: Result<Monitor, ClientError> =
        prepared.start_with_control(&cancel, control.clone());
    assert!(matches!(result, Err(ClientError::UnsupportedFeature)));
    assert!(control.calls.lock().unwrap().is_empty());
}

struct PendingControl;

impl SelectorControl for PendingControl {
    fn ready(&self) -> SelectorControlFuture<'_> {
        Box::pin(std::future::pending())
    }

    fn select<'a>(&'a self, _: &'a str, _: &'a str) -> SelectorControlFuture<'a> {
        Box::pin(std::future::pending())
    }
}

#[test]
fn embedded_control_readiness_cancellation_does_not_wait_for_the_callback_timeout() {
    let prepared = Prepared::for_embedded_profile(&profile()).unwrap().unwrap();
    let signal = Arc::new(vpn_application::client::CancellationSignal::default());
    let cancel = Cancellation::new(Arc::clone(&signal), 1);
    let canceller = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(30));
        signal.cancel(1);
    });
    let started = Instant::now();
    let result = prepared.start_with_control(&cancel, Arc::new(PendingControl));
    canceller.join().unwrap();
    assert!(matches!(result, Err(ClientError::Cancelled)));
    assert!(started.elapsed() < Duration::from_secs(1));
}

#[test]
fn embedded_monitor_stops_without_waiting_for_the_next_observation_interval() {
    let mut profile = profile();
    let ProxyProtocol::Graph(graph) = &mut profile.protocol else {
        unreachable!()
    };
    graph.observatory.as_mut().unwrap().members.clear();
    let prepared = Prepared::for_embedded_profile(&profile).unwrap().unwrap();
    let cancel = Cancellation::new(Arc::default(), 1);
    let mut monitor = prepared
        .start_with_control(&cancel, Arc::new(EmbeddedControl::default()))
        .unwrap();
    let started = Instant::now();
    monitor.stop().unwrap();
    assert!(monitor.is_finished());
    assert!(started.elapsed() < Duration::from_secs(1));
}
