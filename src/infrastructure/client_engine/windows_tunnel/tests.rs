use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::time::{Duration, Instant};

use serde_json::json;
use vpn_application::client::EngineRuntime;
use vpn_application::client::{ClientEnginePort, ClientNormalizerPort};

use super::*;
use crate::client_engine::import_tests::{FixtureRoot, cancellation, trusted_binary};
use crate::client_engine::managed_tunnel::document;
use crate::client_engine::{APPROVED_ENGINE_SHA256, APPROVED_ENGINE_VERSION, TrustedEngineSpec};
use crate::client_normalize::SubscriptionNormalizer;

fn profile(port: u16) -> ProxyProfile {
    let link = format!("socks5://127.0.0.1:{port}#Service-fixture");
    SubscriptionNormalizer
        .normalize(link.as_bytes())
        .unwrap()
        .remove(0)
}

#[test]
#[ignore = "requires the pinned BlazeCore executable and BlazeCoreTun running for the current user"]
fn managed_tun_sends_selected_traffic_and_stops_under_blazer_ownership() {
    let relay = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let target = Ipv4Addr::new(198, 51, 100, 213);
    let port = relay.local_addr().unwrap().port();
    let root = FixtureRoot::new();
    let mut engine = BlazeCoreProcess::new(
        TrustedEngineSpec {
            source_path: trusted_binary(),
            expected_sha256: APPROVED_ENGINE_SHA256.into(),
            expected_version: APPROVED_ENGINE_VERSION.into(),
        },
        root.0.clone(),
        0,
    );
    let cancel = cancellation();
    let profile = profile(port);
    let mut records = document(&profile, None).unwrap();
    records[0]["route_address"] = json!([format!("{target}/32")]);
    records[0]["addresses"] = json!(["192.0.2.185/30"]);
    records[0]["auto_dns"] = json!(false);
    require_running_service().unwrap();
    let directory = engine
        .prepare(&serde_json::to_vec(&records).unwrap(), &cancel)
        .unwrap();
    engine
        .start_config(directory, RunningConnection::Tun, &cancel)
        .unwrap();
    assert_eq!(
        engine.poll(),
        Ok(EngineRuntime::Running(RunningConnection::Tun))
    );

    let runtime_directory = engine.private_directory.as_ref().unwrap().0.clone();
    let runtime_root = root.0.join("runtime");
    let peer = std::thread::spawn(move || {
        relay_once(&relay);
        answer_probe(&relay, &runtime_root);
    });
    let mut connection = TcpStream::connect_timeout(&(target, 8080).into(), Duration::from_secs(3))
        .expect("managed route accepts TCP connection");
    connection
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    connection.write_all(b"blazer").unwrap();
    let mut response = [0; 6];
    connection.read_exact(&mut response).unwrap();
    assert_eq!(&response, b"blazer");
    drop(connection);

    let url = format!("http://{target}:8080/generate_204");
    engine
        .probe_tunnel_at(&profile, Some(&profile), false, &cancel, &url)
        .expect("active TUN probe reaches the HTTP target without another engine");
    peer.join().unwrap();
    assert_eq!(
        engine.poll(),
        Ok(EngineRuntime::Running(RunningConnection::Tun))
    );
    assert_eq!(
        engine.private_directory.as_ref().unwrap().0,
        runtime_directory,
        "probe preserves the active engine runtime"
    );

    engine.stop().unwrap();
    assert_eq!(engine.poll(), Ok(EngineRuntime::Stopped));
}

fn accept_socks(listener: &TcpListener) -> TcpStream {
    listener.set_nonblocking(true).unwrap();
    let deadline = Instant::now() + Duration::from_secs(8);
    let mut stream = loop {
        match listener.accept() {
            Ok((stream, _)) => break stream,
            Err(error)
                if error.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => panic!("managed TUN never reached the SOCKS fixture: {error}"),
        }
    };
    stream.set_nonblocking(false).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let mut greeting = [0; 3];
    stream.read_exact(&mut greeting).unwrap();
    assert_eq!(greeting, [5, 1, 0]);
    stream.write_all(&[5, 0]).unwrap();
    let mut request = [0; 10];
    stream.read_exact(&mut request).unwrap();
    assert_eq!(request, [5, 1, 0, 1, 198, 51, 100, 213, 31, 144]);
    stream.write_all(&[5, 0, 0, 1, 127, 0, 0, 1, 0, 0]).unwrap();
    stream
}

fn relay_once(listener: &TcpListener) {
    let mut stream = accept_socks(listener);
    let mut payload = [0; 6];
    stream.read_exact(&mut payload).unwrap();
    stream.write_all(&payload).unwrap();
}

fn answer_probe(listener: &TcpListener, runtime_root: &std::path::Path) {
    let mut stream = accept_socks(listener);
    let mut request = String::new();
    BufReader::new(&mut stream)
        .take(4096)
        .read_line(&mut request)
        .unwrap();
    assert_eq!(request, "GET /generate_204 HTTP/1.1\r\n");
    let engines = std::fs::read_dir(runtime_root)
        .unwrap()
        .map(|entry| entry.unwrap())
        .filter(|entry| entry.file_type().unwrap().is_dir())
        .count();
    assert_eq!(
        engines, 1,
        "active TUN probe must not launch another engine"
    );
    stream
        .write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
        .unwrap();
}
