use super::*;

fn adapter(index: u32, component: &str) -> Adapter {
    Adapter {
        index,
        name: format!("interface-{index}"),
        description: "Tunnel adapter".into(),
        interface_type: 53,
        connected: true,
        hardware: false,
        component: component.into(),
        driver: String::new(),
    }
}

#[test]
fn tun_preflight_classifies_only_active_forwarding_vpn_interfaces() {
    let mut snapshot = Snapshot::default();
    let mut wifi = adapter(1, "intel-wifi");
    wifi.hardware = true;
    wifi.interface_type = 71;
    let mut hyperv = adapter(2, "vms_mp");
    hyperv.interface_type = 6;
    let mut disconnected = adapter(3, "Wintun");
    disconnected.connected = false;
    snapshot.adapters = vec![wifi, hyperv, disconnected, adapter(4, "Wintun")];
    snapshot.routes = (1..=3)
        .map(|index| Route {
            index,
            prefix: "0.0.0.0/0".into(),
        })
        .collect();
    assert!(
        conflict(&snapshot).unwrap().is_none(),
        "physical/Hyper-V/disconnected/stale tunnels pass"
    );
    snapshot.routes.push(Route {
        index: 4,
        prefix: "fe80::/64".into(),
    });
    assert!(conflict(&snapshot).unwrap().is_none());
    snapshot.routes.push(Route {
        index: 4,
        prefix: "10.42.0.8/32".into(),
    });
    let detail = conflict(&snapshot).unwrap().unwrap();
    assert_eq!(detail.name(), "interface-4");
    assert_eq!(detail.kind(), TunAdapterKind::Tunnel);
    assert!(!format!("{:?}", ClientError::TunConflict(detail)).contains("interface-4"));
    snapshot.adapters[3].name = "BlazeCore".into();
    assert!(
        conflict(&snapshot).unwrap().is_some(),
        "names never grant ownership exemption"
    );
    snapshot.adapters[3].connected = false;
    snapshot.ras.push(RasConnection {
        name: "connected-ras".into(),
        description: "Ikev2".into(),
    });
    assert_eq!(
        conflict(&snapshot).unwrap().unwrap().kind(),
        TunAdapterKind::WindowsVpn
    );
    let bounded = TunConflict::new(
        &format!("{}\n\u{202e}", "я".repeat(70)),
        "driver\r\n",
        TunAdapterKind::Tunnel,
    );
    assert!(bounded.name().len() <= 96);
    assert_eq!(bounded.description(), "driver");
    let mut pppoe = adapter(5, "ms_pppoeminiport");
    pppoe.interface_type = 23;
    assert_eq!(vpn_kind(&pppoe), None);
    snapshot.routes.push(Route {
        index: 99,
        prefix: "not-a-route".into(),
    });
    assert_eq!(
        conflict(&snapshot),
        Err(ClientError::TunPreflightUnavailable)
    );
}

#[test]
#[ignore = "read-only host acceptance; requires an already active external VPN"]
fn tun_preflight_reports_current_host_conflict_without_starting_core() {
    use crate::client_engine::{BlazeCoreProcess, TrustedEngineSpec};
    use vpn_application::client::{CancellationSignal, ClientEnginePort, ClientNormalizerPort};
    use vpn_domain::client::ConnectionMode;
    let signal = std::sync::Arc::new(CancellationSignal::default());
    let cancel = Cancellation::new(signal, 1);
    let mut engine = BlazeCoreProcess::new(
        TrustedEngineSpec {
            source_path: "Z:/nonexistent-engine.exe".into(),
            expected_sha256: String::new(),
            expected_version: String::new(),
        },
        "Z:/nonexistent-preflight-data".into(),
        0,
    );
    let profile = crate::client_normalize::SubscriptionNormalizer
        .normalize(b"socks5://127.0.0.1:1080#Preflight")
        .unwrap()
        .remove(0);
    match engine.start(ConnectionMode::Tun, &profile, 0, &cancel) {
        Err(ClientError::TunConflict(detail)) => println!(
            "TUN_PREFLIGHT_BLOCKED adapter={} description={}",
            detail.name(),
            detail.description()
        ),
        result => panic!(
            "expected external VPN conflict before service/core/filesystem access: {result:?}"
        ),
    }
    assert!(engine.child.is_none());
    assert!(engine.private_directory.is_none());
}
