use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, TcpListener};
use std::path::PathBuf;
use std::sync::{Arc, mpsc};

use vpn_application::client::{CancellationSignal, ClientNormalizerPort};

use super::*;
use crate::client_engine::TrustedEngineSpec;
use crate::client_normalize::SubscriptionNormalizer;
use vpn_domain::client::ProbeStrategy;

fn wireguard(amnezia: bool) -> ProxyProfile {
    let obfuscation = if amnezia {
        "Jc = 2\nJmin = 12\nJmax = 34\nS1 = 12\nS2 = 13\nH1 = 101\nH2 = 201\nH3 = 301\nH4 = 401\n"
    } else {
        ""
    };
    let input = format!(
        "[Interface]\nPrivateKey = KioqKioqKioqKioqKioqKioqKioqKioqKioqKioqKio=\nAddress = 10.77.0.2/32\n{obfuscation}[Peer]\nPublicKey = F9firNML9eGdhhroCYjRf66PZi9dgLAFP2zHPNqx0mQ=\nAllowedIPs = 0.0.0.0/0\nEndpoint = 127.0.0.1:51820\n"
    );
    SubscriptionNormalizer
        .normalize(input.as_bytes())
        .unwrap()
        .remove(0)
}

#[test]
fn graph_probe_detects_wireguard_identity_shared_with_active_profile() {
    use vpn_domain::client::{GraphOutbound, GraphOutboundKind, GraphTarget, ProxyGraph};
    let member = wireguard(true);
    let mut graph = member.clone();
    graph.protocol = ProxyProtocol::Graph(Box::new(ProxyGraph {
        outbounds: vec![GraphOutbound {
            tag: "shared".into(),
            kind: GraphOutboundKind::Proxy(Box::new(member.clone())),
        }],
        groups: vec![],
        rules: vec![],
        default_target: GraphTarget::Outbound("shared".into()),
        dns: vpn_domain::routing::DnsPolicy::Automatic,
        observatory: None,
    }));
    assert!(same_wireguard_identity(&graph, &member));
    assert!(same_wireguard_identity(&member, &graph));
    assert!(same_wireguard_identity(&graph, &graph));
}

fn active_tunnel() -> BlazeCoreProcess {
    let mut process = BlazeCoreProcess::new(
        TrustedEngineSpec {
            source_path: PathBuf::from("intentionally-unavailable-test-engine"),
            expected_sha256: String::new(),
            expected_version: String::new(),
        },
        PathBuf::from("intentionally-unused-probe-directory"),
        0,
    );
    process.connection = Some(RunningConnection::Tun);
    process
}

fn cancellation() -> Cancellation {
    Cancellation::new(Arc::default(), 1)
}

fn accept_probe(listener: &TcpListener) -> std::net::TcpStream {
    listener.set_nonblocking(true).unwrap();
    let deadline = Instant::now() + Duration::from_secs(7);
    loop {
        if let Ok((stream, _)) = listener.accept() {
            stream.set_nonblocking(false).unwrap();
            stream.set_nodelay(true).unwrap();
            return stream;
        }
        assert!(
            Instant::now() < deadline,
            "probe never reached the OS network path"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn endpoint(status: &str) -> (String, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let url = format!("http://{}/generate_204", listener.local_addr().unwrap());
    let reply = format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
    let worker = std::thread::spawn(move || {
        let mut stream = accept_probe(&listener);
        stream
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let mut request = [0u8; 2048];
        let length = stream.read(&mut request).unwrap();
        assert!(request[..length].starts_with(b"GET /generate_204 HTTP/1.1\r\n"));
        stream.write_all(reply.as_bytes()).unwrap();
    });
    (url, worker)
}

#[test]
fn active_wireguard_and_amnezia_probe_uses_os_route_without_second_handshake() {
    for amnezia in [false, true] {
        let process = active_tunnel();
        let profile = wireguard(amnezia);
        let (url, worker) = endpoint("204 No Content");
        let result =
            process.probe_tunnel_at(&profile, Some(&profile), false, &cancellation(), &url);
        worker.join().unwrap();
        assert!(result.is_ok(), "active TUN probe failed: {result:?}");
        assert_eq!(process.connection, Some(RunningConnection::Tun));
        assert!(process.child.is_none(), "probe must not start an engine");
        assert!(process.private_directory.is_none());
    }
}

#[test]
fn active_tunnel_does_not_test_another_profile_through_a_nested_connection() {
    let process = active_tunnel();
    let active = wireguard(true);
    let mut other = active.clone();
    other.profile_uuid = "another-server".into();
    assert_eq!(
        process.probe_tunnel_at(
            &other,
            Some(&active),
            true,
            &cancellation(),
            "http://127.0.0.1:9"
        ),
        Err(ClientError::DisconnectFirst)
    );
    assert_eq!(
        process.probe_tunnel_at(&active, None, true, &cancellation(), "http://127.0.0.1:9"),
        Err(ClientError::DisconnectFirst)
    );
}

#[test]
fn active_tunnel_probe_reports_http_failure_without_a_successful_ping() {
    let process = active_tunnel();
    let profile = wireguard(true);
    let (url, worker) = endpoint("503 Service Unavailable");
    let result = process.probe_tunnel_at(&profile, Some(&profile), true, &cancellation(), &url);
    worker.join().unwrap();
    assert_eq!(result, Err(ClientError::Network));
}

#[test]
fn active_tunnel_probe_can_be_cancelled_while_waiting_for_the_response() {
    let process = active_tunnel();
    let profile = wireguard(true);
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let url = format!("http://{}/generate_204", listener.local_addr().unwrap());
    let signal = Arc::new(CancellationSignal::default());
    let cancel = Cancellation::new(Arc::clone(&signal), 1);
    let (done, release) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        let mut stream = accept_probe(&listener);
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut request = [0u8; 2048];
        assert!(stream.read(&mut request).unwrap() > 0);
        signal.cancel(1);
        let _ = release.recv_timeout(Duration::from_secs(2));
    });
    let started = Instant::now();
    let result = process.probe_tunnel_at(&profile, Some(&profile), false, &cancel, &url);
    let _ = done.send(());
    worker.join().unwrap();
    assert_eq!(result, Err(ClientError::Cancelled));
    assert!(started.elapsed() < Duration::from_secs(2));
}

fn keep_alive_endpoint(
    method: &str,
    delays: &[u64],
    body: &[u8],
) -> (String, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let url = format!(
        "http://{}/custom/ping?check=1",
        listener.local_addr().unwrap()
    );
    let request_line = format!("{method} /custom/ping?check=1 HTTP/1.1\r\n");
    let body = body.to_owned();
    let delays = delays.to_owned();
    let head = method == "HEAD";
    let worker = std::thread::spawn(move || {
        let stream = accept_probe(&listener);
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut stream = BufReader::new(stream);
        for delay in delays {
            let mut first = String::new();
            stream.read_line(&mut first).unwrap();
            assert_eq!(
                first, request_line,
                "all attempts must reuse the warmed connection"
            );
            loop {
                let mut header = String::new();
                assert!(stream.read_line(&mut header).unwrap() > 0);
                if header == "\r\n" {
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(delay));
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: keep-alive\r\n\r\n",
                body.len()
            );
            stream.get_mut().write_all(reply.as_bytes()).unwrap();
            if !head {
                stream.get_mut().write_all(&body).unwrap();
            }
        }
    });
    (url, worker)
}

#[test]
fn configured_head_and_custom_url_reuse_the_warmed_http_connection() {
    let process = active_tunnel();
    let profile = wireguard(true);
    let (url, worker) = keep_alive_endpoint("HEAD", &[0, 0, 0], b"not-sent-for-head");
    let settings = ProbeSettings {
        http_method: ProbeHttpMethod::Head,
        url,
        attempts: 2,
        warmup: true,
        ..ProbeSettings::default()
    };
    let result = process.probe_tunnel_with_settings(
        &profile,
        Some(&profile),
        false,
        &settings,
        &cancellation(),
    );
    worker.join().unwrap();
    assert!(result.is_ok(), "configured HEAD probe failed: {result:?}");
}

#[test]
fn get_consumes_success_body_and_discards_warmup_from_average() {
    let process = active_tunnel();
    let profile = wireguard(false);
    let (url, worker) = keep_alive_endpoint("GET", &[300, 10, 20], b"ready");
    let settings = ProbeSettings {
        http_method: ProbeHttpMethod::Get,
        url,
        attempts: 2,
        warmup: true,
        strategy: ProbeStrategy::Average,
        ..ProbeSettings::default()
    };
    let result = process.probe_tunnel_with_settings(
        &profile,
        Some(&profile),
        false,
        &settings,
        &cancellation(),
    );
    worker.join().unwrap();
    let elapsed = result.unwrap();
    assert!(elapsed >= Duration::from_millis(10));
    assert!(
        elapsed < Duration::from_millis(100),
        "warmup must not enter the average: {elapsed:?}"
    );
}

#[test]
fn configured_timeout_applies_while_waiting_for_the_http_response() {
    let process = active_tunnel();
    let profile = wireguard(true);
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let url = format!("http://{}/generate_204", listener.local_addr().unwrap());
    let (done, release) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        let mut stream = accept_probe(&listener);
        stream
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let mut request = [0u8; 2048];
        assert!(stream.read(&mut request).unwrap() > 0);
        let _ = release.recv_timeout(Duration::from_secs(2));
    });
    let settings = ProbeSettings {
        url,
        timeout_ms: 250,
        warmup: false,
        ..ProbeSettings::default()
    };
    let started = Instant::now();
    let result = process.probe_tunnel_with_settings(
        &profile,
        Some(&profile),
        false,
        &settings,
        &cancellation(),
    );
    let _ = done.send(());
    worker.join().unwrap();
    assert_eq!(result, Err(ClientError::Timeout));
    assert!(started.elapsed() < Duration::from_secs(1));
}

#[test]
fn failed_warmup_aborts_without_publishing_a_latency() {
    let process = active_tunnel();
    let profile = wireguard(true);
    let (url, worker) = endpoint("503 Service Unavailable");
    let settings = ProbeSettings {
        url,
        http_method: ProbeHttpMethod::Get,
        warmup: true,
        attempts: 5,
        ..ProbeSettings::default()
    };
    let result = process.probe_tunnel_with_settings(
        &profile,
        Some(&profile),
        false,
        &settings,
        &cancellation(),
    );
    worker.join().unwrap();
    assert_eq!(result, Err(ClientError::Network));
}

fn answer_head_requests(
    stream: std::net::TcpStream,
    delays: &[u64],
    close: bool,
) -> std::net::TcpStream {
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let mut stream = BufReader::new(stream);
    for delay in delays {
        let mut first = String::new();
        assert!(stream.read_line(&mut first).unwrap() > 0);
        assert!(first.starts_with("HEAD /check HTTP/1.1\r\n"));
        loop {
            let mut header = String::new();
            assert!(stream.read_line(&mut header).unwrap() > 0);
            if header == "\r\n" {
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(*delay));
        let connection = if close { "close" } else { "keep-alive" };
        stream.get_mut().write_all(format!("HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: {connection}\r\n\r\n").as_bytes()).unwrap();
    }
    stream.into_inner()
}

#[test]
fn a_server_that_closes_every_connection_cannot_publish_cold_samples_as_warm() {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let url = format!("http://{}/check", listener.local_addr().unwrap());
    let (done, release) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        // One warm-up plus the configured count + 1 discarded cold requests.
        let mut responses = Vec::new();
        for _ in 0..4 {
            responses.push(answer_head_requests(accept_probe(&listener), &[0], true));
        }
        // Connection: close makes the client open a new connection. Keep the
        // fixture sockets alive until it has classified the final response;
        // tearing down the Windows listener here can race delivery with RST.
        let _ = release.recv_timeout(Duration::from_secs(7));
        drop(responses);
    });
    let settings = ProbeSettings {
        url,
        attempts: 2,
        warmup: true,
        ..ProbeSettings::default()
    };
    let process = active_tunnel();
    let profile = wireguard(true);
    let result = process.probe_tunnel_with_settings(
        &profile,
        Some(&profile),
        false,
        &settings,
        &cancellation(),
    );
    let _ = done.send(());
    worker.join().unwrap();
    assert_eq!(result, Err(ClientError::ProbeConnectionNotReusable));
}

#[test]
fn reconnect_discards_the_new_handshake_and_rewarms_before_measuring() {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let url = format!("http://{}/check", listener.local_addr().unwrap());
    let worker = std::thread::spawn(move || {
        answer_head_requests(accept_probe(&listener), &[300], true);
        answer_head_requests(accept_probe(&listener), &[300, 10, 20], false);
    });
    let settings = ProbeSettings {
        url,
        attempts: 2,
        warmup: true,
        strategy: ProbeStrategy::Average,
        ..ProbeSettings::default()
    };
    let process = active_tunnel();
    let profile = wireguard(true);
    let result = process.probe_tunnel_with_settings(
        &profile,
        Some(&profile),
        false,
        &settings,
        &cancellation(),
    );
    worker.join().unwrap();
    let elapsed = result.unwrap();
    assert!(elapsed >= Duration::from_millis(10));
    assert!(
        elapsed < Duration::from_millis(100),
        "cold reconnect entered the average: {elapsed:?}"
    );
}
