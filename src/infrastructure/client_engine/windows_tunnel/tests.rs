use std::io;
use std::net::{SocketAddrV4, TcpListener};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use etherparse::{NetSlice, PacketBuilder, SlicedPacket, TransportSlice};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::mpsc;
use vpn_application::client::{CancellationSignal, ClientNormalizerPort};
use vpn_domain::client::{ProxyProtocol, ProxySecurity};

use super::*;
use crate::client_engine::{APPROVED_ENGINE_SHA256, APPROVED_ENGINE_VERSION, TrustedEngineSpec};

const TARGET: [u8; 4] = [198, 18, 0, 44];
const TARGET_V6: Ipv6Addr = Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 0x44);
const PORT: u16 = 19876;
const DATA: &[u8] = b"windows-tun\0payload";
const DNS_PORT: u16 = 53;
const DNS_QUERY: &[u8] =
    b"\x79\x31\x01\x00\x00\x01\x00\x00\x00\x00\x00\x00\x07fixture\x07invalid\x00\x00\x01\x00\x01";

fn dns_answer() -> Vec<u8> {
    let mut reply = DNS_QUERY.to_vec();
    reply[2..4].copy_from_slice(&[0x81, 0x80]);
    reply[7] = 1;
    reply.extend_from_slice(b"\xc0\x0c\x00\x01\x00\x01\x00\x00\x00\x3c\x00\x04\xc6\x12\x00\x2c");
    reply
}

fn cancel() -> Cancellation {
    Cancellation::new(Arc::new(CancellationSignal::default()), 1)
}

fn profile(endpoint: SocketAddr) -> ProxyProfile {
    ProxyProfile {
        profile_uuid: uuid::Uuid::new_v4().to_string(),
        name: "Windows TCP fixture".into(),
        host: endpoint.ip().to_string(),
        port: endpoint.port(),
        protocol: ProxyProtocol::Http {
            username: None,
            password: None,
        },
        transport: ProxyTransport::Tcp,
        security: ProxySecurity::None,
    }
}

#[test]
fn pinning_a_server_address_preserves_websocket_authority_and_tls_name() {
    let mut original = profile("127.0.0.1:443".parse().unwrap());
    original.host = "example.invalid".into();
    original.security = ProxySecurity::Tls {
        server_name: original.host.clone(),
        allow_insecure: false,
        alpn: vpn_domain::client::AlpnProtocols::new(vec!["http/1.1".into()]).unwrap(),
    };
    original.transport = ProxyTransport::WebSocket {
        path: "/tunnel".into(),
        host: None,
        early_data: None,
    };
    let pinned = pin_address(&original, "192.0.2.2".parse().unwrap());
    assert_eq!(pinned.host, "192.0.2.2");
    assert_eq!(pinned.security, original.security);
    assert_eq!(
        pinned.transport,
        ProxyTransport::WebSocket {
            path: "/tunnel".into(),
            host: Some("example.invalid:443".into()),
            early_data: None,
        }
    );
}

#[test]
fn bypass_covers_only_the_upstream_host() {
    let peer = "192.0.2.44".parse().unwrap();
    let route = |prefix: &str| BestRoute {
        interface_index: 7,
        destination_prefix: prefix.to_owned(),
        next_hop: "192.0.2.1".parse().unwrap(),
    };
    let exception = bypass_route(peer, route("0.0.0.0/0")).unwrap().unwrap();
    assert_eq!(
        (exception.destination, exception.prefix, exception.ifindex),
        (peer, 32, Some(7))
    );
    assert!(bypass_route(peer, route("192.0.2.0/24")).unwrap().is_none());
    let captures = capture_routes(13);
    assert!(
        captures
            .iter()
            .all(|route| route.prefix == 1 && route.ifindex == Some(13))
    );
    assert_eq!(
        captures
            .iter()
            .filter(|route| route.destination.is_ipv6())
            .count(),
        2
    );
}

#[test]
fn tunnel_readiness_requires_a_borrowed_route_to_remain_present() {
    let captures = capture_routes(13);
    let borrowed = Route::new("192.0.2.44".parse().unwrap(), 32).with_ifindex(7);
    let required = || captures.iter().chain(std::iter::once(&borrowed));
    let mut observed = captures.to_vec();
    assert!(!routes_present(required(), &observed));
    observed.push(borrowed.clone().with_ifindex(8));
    assert!(!routes_present(required(), &observed));
    observed.push(borrowed.clone().with_gateway(Ipv4Addr::UNSPECIFIED.into()));
    assert!(routes_present(required(), &observed));
}

#[test]
fn route_monitor_refreshes_only_after_a_change_and_detects_lost_route() {
    let runtime = Builder::new_current_thread().build().unwrap();
    let captures = capture_routes(13);
    let borrowed = Route::new("192.0.2.44".parse().unwrap(), 32).with_ifindex(7);
    let required: Vec<_> = captures.iter().cloned().chain([borrowed]).collect();
    let monitor = RouteMonitor {
        required: required.clone(),
        changed: Arc::new(AtomicBool::new(true)),
        listener: runtime.spawn(std::future::pending()),
    };
    assert!(monitor.healthy(|| Ok(required.clone())));
    assert!(monitor.healthy(|| panic!("route table should stay cached")));
    monitor.changed.store(true, Ordering::Release);
    monitor.changed.store(true, Ordering::Release);
    assert!(monitor.healthy(|| Ok(required)));
    assert!(monitor.healthy(|| panic!("successive changes should share one read")));
    monitor.changed.store(true, Ordering::Release);
    assert!(!monitor.healthy(|| Ok(captures.to_vec())));
}

#[test]
#[ignore = "read-only query of the host Windows TCP/IP module; requires IPv4 and IPv6 loopback"]
fn interface_readiness_query_accepts_preferred_dual_stack_loopback() {
    let cancellation = cancel();
    let route = selected_route(Ipv4Addr::LOCALHOST.into(), &cancellation).unwrap();
    let query = interface_readiness_script(
        route.interface_index,
        Ipv4Addr::LOCALHOST,
        Ipv6Addr::LOCALHOST,
    );
    run_bounded(
        powershell(&query).unwrap(),
        &cancellation,
        ClientError::EngineReadiness,
    )
    .unwrap();

    let runtime = Builder::new_multi_thread().enable_all().build().unwrap();
    let routes = Arc::new(Handle::new().unwrap());
    let routes_to_loopback: Vec<_> = runtime
        .block_on(routes.list())
        .unwrap()
        .into_iter()
        .filter(|entry| entry.ifindex == Some(route.interface_index))
        .collect();
    assert!(!routes_to_loopback.is_empty());
    let monitor = RouteMonitor::start(&runtime, Arc::clone(&routes), routes_to_loopback)
        .expect("subscribe to Windows route notifications");
    assert!(monitor.healthy(|| runtime.block_on(routes.list())));
}

#[test]
fn matching_digest_alone_does_not_trust_an_unsigned_driver() {
    let root = std::env::temp_dir().join(format!("vpn-windows-unsigned-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&root).unwrap();
    let path = root.join("unsigned.dll");
    fs::write(&path, b"unsigned test fixture").unwrap();
    let driver = Driver {
        sha256: file_sha256(&path).unwrap(),
        path: path.clone(),
        bundled_version: None,
    };
    assert!(matches!(
        driver.stage(&root, &cancel()),
        Err(ClientError::EngineUntrusted)
    ));
    assert_eq!(fs::read_dir(root.join("runtime")).unwrap().count(), 0);
    fs::remove_file(path).unwrap();
    fs::remove_dir(root.join("runtime")).unwrap();
    fs::remove_dir(root).unwrap();
}

#[test]
#[ignore = "requires a bundled Wintun DLL or signed override via VPN_CLIENT_WINTUN_*; does not activate Wintun"]
fn configured_wintun_library_passes_staging_and_cleanup() {
    let root = std::env::temp_dir().join(format!("vpn-driver-stage-{}", uuid::Uuid::new_v4()));
    let driver =
        Driver::configured().expect("provide the bundled Wintun DLL or a trusted override");
    let staged = driver.stage(&root, &cancel()).expect("approved signed DLL");
    assert_eq!(
        file_sha256(&staged.0.join("wintun.dll")).unwrap(),
        driver.sha256
    );
    drop(staged);
    remove_test_root(root);
}

#[cfg(target_arch = "x86_64")]
#[test]
fn wintun_discovery_uses_existing_bundle_unless_an_override_is_present() {
    let root = std::env::temp_dir().join(format!("vpn-wintun-discovery-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&root).unwrap();
    let executable = root.join("vpn-client.exe");
    let bundle = root.join("wintun.dll");
    let external = root.join("override.dll");
    let resolve = |path, hash| Driver::resolve(path, hash, Some(&executable));

    assert!(resolve(None, None).is_none());
    fs::create_dir(&bundle).unwrap();
    assert!(resolve(None, None).is_none());
    fs::remove_dir(&bundle).unwrap();
    fs::write(&bundle, b"discovery only").unwrap();

    let selected = resolve(None, None).unwrap();
    assert_eq!(selected.path, bundle);
    assert_eq!(selected.sha256, APPROVED_WINTUN_SHA256);
    assert_eq!(selected.bundled_version, Some(BUNDLED_WINTUN_VERSION));

    let lower_hash = OsString::from(APPROVED_WINTUN_SHA256.to_ascii_lowercase());
    let override_path = external.clone().into_os_string();
    let selected = resolve(Some(override_path.clone()), Some(lower_hash.clone())).unwrap();
    assert_eq!(selected.path, external);
    assert_eq!(selected.sha256, APPROVED_WINTUN_SHA256);
    assert_eq!(selected.bundled_version, None);

    assert!(resolve(Some(override_path.clone()), None).is_none());
    assert!(resolve(None, Some(lower_hash.clone())).is_none());
    assert!(resolve(Some(OsString::new()), Some(lower_hash.clone())).is_none());
    assert!(resolve(Some("relative.dll".into()), Some(lower_hash)).is_none());
    assert!(resolve(Some(override_path), Some("not-a-sha256".into())).is_none());

    fs::remove_file(bundle).unwrap();
    fs::remove_dir(root).unwrap();
}

#[test]
#[ignore = "changes Windows routes during a real Wintun session; requires elevation and approved Wintun and Shoes"]
fn windows_system_tun_crosses_wintun() {
    let cancellation = cancel();
    require_elevation(&cancellation).expect("Windows TUN needs an elevated process");
    let target = SocketAddr::from((TARGET, PORT));
    let target_v6 = SocketAddr::from((TARGET_V6, PORT));
    let route_before = selected_route(target.ip(), &cancellation).expect("initial Windows route");
    let route_v6_before = selected_route(target_v6.ip(), &cancellation).ok();
    let root = std::env::temp_dir().join(format!("vpn-system-tun-{}", uuid::Uuid::new_v4()));
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let mut engine = WindowsEngine::new(fixture_shoes(root.clone()));
    engine
        .start(
            ConnectionMode::Tun,
            &profile(listener.local_addr().unwrap()),
            0,
            &cancellation,
        )
        .expect("start system tunnel");
    assert_eq!(
        engine.poll().unwrap(),
        EngineRuntime::Running(RunningConnection::Tun)
    );

    let route_inside =
        selected_route(target.ip(), &cancellation).expect("route with Wintun active");
    assert_ne!(route_before.interface_index, route_inside.interface_index);
    let route_v6_inside =
        selected_route(target_v6.ip(), &cancellation).expect("IPv6 route with Wintun active");
    assert_eq!(
        route_v6_inside.interface_index,
        route_inside.interface_index
    );
    let captures = capture_routes(route_inside.interface_index);
    let owned = &engine.tunnel.as_ref().expect("active tunnel").owned;
    assert!(
        captures
            .iter()
            .all(|route| owned.iter().any(|entry| same_route(entry, route)))
    );
    exchange_routed_payload(listener.try_clone().unwrap(), target, TUN_ADDRESS.into());
    exchange_routed_payload(
        listener.try_clone().unwrap(),
        target_v6,
        TUN_IPV6_ADDRESS.into(),
    );
    exchange_routed_dns(listener);

    let tunnel = engine.tunnel.as_ref().unwrap();
    tunnel
        .runtime
        .block_on(tunnel.routes.delete(&captures[0]))
        .expect("simulate loss of an owned capture route");
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while engine.poll().unwrap() != EngineRuntime::Stopped {
        assert!(
            std::time::Instant::now() < deadline,
            "still connected after route loss"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(engine.poll().unwrap(), EngineRuntime::Stopped);
    assert_eq!(
        selected_route(target.ip(), &cancellation).unwrap(),
        route_before
    );
    assert_eq!(
        selected_route(target_v6.ip(), &cancellation).ok(),
        route_v6_before
    );
    assert_capture_routes_removed(&captures);
    remove_test_root(root);
}

fn exchange_routed_payload(listener: TcpListener, target: SocketAddr, source: IpAddr) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let runtime = Builder::new_current_thread().enable_all().build().unwrap();
    runtime.block_on(async {
        let listener = tokio::net::TcpListener::from_std(nonblocking(listener)).unwrap();
        let client = async {
            let mut stream = tokio::net::TcpStream::connect(target).await.unwrap();
            assert_eq!(stream.local_addr().unwrap().ip(), source);
            stream.write_all(DATA).await.unwrap();
            let mut echo = [0; DATA.len()];
            stream.read_exact(&mut echo).await.unwrap();
            assert_eq!(echo, DATA);
        };
        tokio::time::timeout(Duration::from_secs(8), async {
            tokio::join!(echo_connect(&listener, target), client);
        })
        .await
        .expect("route traffic through Wintun and Shoes before timeout");
    });
}

fn assert_capture_routes_removed(captures: &[Route]) {
    let runtime = Builder::new_current_thread().enable_all().build().unwrap();
    let current = runtime.block_on(Handle::new().unwrap().list()).unwrap();
    for route in captures {
        assert!(!current.iter().any(|entry| same_route(entry, route)));
    }
}

fn exchange_routed_dns(listener: TcpListener) {
    let runtime = Builder::new_current_thread().enable_all().build().unwrap();
    runtime.block_on(async {
        let listener = tokio::net::TcpListener::from_std(nonblocking(listener)).unwrap();
        let client = async {
            let socket = tokio::net::UdpSocket::bind((TUN_ADDRESS, 0)).await.unwrap();
            socket
                .connect(SocketAddr::from((TARGET, DNS_PORT)))
                .await
                .unwrap();
            socket.send(DNS_QUERY).await.unwrap();
            let mut response = [0; 512];
            let length = socket.recv(&mut response).await.unwrap();
            assert_eq!(&response[..length], dns_answer());
        };
        tokio::time::timeout(Duration::from_secs(8), async {
            tokio::join!(echo_dns(&listener, dns_proxy_target()), client);
        })
        .await
        .expect("route UDP DNS through Wintun and Shoes before timeout");
    });
}

#[test]
#[ignore = "requires the pinned VPN_CLIENT_TEST_SHOES_EXE Windows binary"]
fn packet_bridge_carries_tcp_v4_v6_and_dns_through_real_shoes() {
    let root = std::env::temp_dir().join(format!("vpn-windows-packets-{}", uuid::Uuid::new_v4()));
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let mut shoes = fixture_shoes(root.clone());
    shoes
        .start(
            ConnectionMode::LocalProxy,
            &profile(listener.local_addr().unwrap()),
            0,
            &cancel(),
        )
        .unwrap();
    let EngineRuntime::Running(RunningConnection::LocalProxy(proxy)) = shoes.poll().unwrap() else {
        panic!("missing real Shoes loopback endpoint");
    };
    let runtime_files = &shoes.private_directory.as_ref().unwrap().0;
    assert!(!runtime_files.join("profile.yaml").try_exists().unwrap());
    let runtime = Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let listener = tokio::net::TcpListener::from_std(nonblocking(listener)).unwrap();
        let target = SocketAddr::from((TARGET, PORT));
        verify_tcp_packet_flow(&listener, proxy, target, TUN_ADDRESS.into()).await;
        let target = SocketAddr::from((TARGET_V6, PORT));
        verify_tcp_packet_flow(&listener, proxy, target, TUN_IPV6_ADDRESS.into()).await;
        tokio::time::timeout(Duration::from_secs(8), async {
            let target = SocketAddrV4::new(TARGET.into(), DNS_PORT);
            let answer = dns_answer();
            tokio::join!(
                echo_dns(&listener, dns_proxy_target()),
                exercise_udp_packet(proxy, target, DNS_QUERY, &answer)
            );
        })
        .await
        .expect("packet DNS exchange completed before timeout");
    });
    shoes.stop().unwrap();
    remove_test_root(root);
}

fn fixture_shoes(root: PathBuf) -> ShoesProcess {
    let binary =
        PathBuf::from(std::env::var_os("VPN_CLIENT_TEST_SHOES_EXE").expect("pinned engine"));
    ShoesProcess::new(
        TrustedEngineSpec {
            source_path: binary.canonicalize().unwrap(),
            expected_sha256: APPROVED_ENGINE_SHA256.into(),
            expected_version: APPROVED_ENGINE_VERSION.into(),
        },
        root,
        0,
    )
}

#[test]
#[ignore = "requires the pinned Windows Shoes binary and the isolated local Hysteria2 UDP fixture on port 18474"]
fn hysteria2_preserves_udp_datagrams_through_windows_packet_bridge() {
    let pin =
        std::env::var("VPN_CLIENT_TEST_HYSTERIA2_PIN").expect("local fixture certificate SHA-256");
    let obfs = if std::env::var_os("VPN_CLIENT_TEST_HYSTERIA2_SALAMANDER").is_some() {
        "&obfs=salamander&obfs-password=fixture-obfs"
    } else {
        ""
    };
    let uri = format!(
        "hy2://fixture:userpass@127.0.0.1:18474/?sni=localhost&insecure=1&pinSHA256={pin}{obfs}#UDP"
    );
    let profile = crate::client_normalize::SubscriptionNormalizer
        .normalize(uri.as_bytes())
        .unwrap()
        .remove(0);
    let root = std::env::temp_dir().join(format!("vpn-hysteria2-udp-{}", uuid::Uuid::new_v4()));
    let mut shoes = fixture_shoes(root.clone());
    shoes
        .start(ConnectionMode::LocalProxy, &profile, 0, &cancel())
        .unwrap();
    let EngineRuntime::Running(RunningConnection::LocalProxy(proxy)) = shoes.poll().unwrap() else {
        panic!("missing loopback endpoint");
    };
    let runtime = Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        tokio::time::timeout(
            Duration::from_secs(8),
            exercise_udp_packet(
                proxy,
                SocketAddrV4::new(Ipv4Addr::new(198, 18, 0, 47), 19879),
                b"windows-hy2-udp\0request",
                b"echo:windows-hy2-udp\0request",
            ),
        )
        .await
        .expect("Hysteria2 UDP exchange completed before timeout");
    });
    shoes.stop().unwrap();
    remove_test_root(root);
}

fn remove_test_root(root: PathBuf) {
    fs::remove_dir(root.join("runtime")).unwrap();
    fs::remove_dir(root).unwrap();
}

async fn accept_connect(
    listener: &tokio::net::TcpListener,
    target: SocketAddr,
) -> tokio::io::BufReader<tokio::net::TcpStream> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
    let (stream, _) = listener.accept().await.unwrap();
    let mut stream = tokio::io::BufReader::new(stream);
    let mut line = String::new();
    stream.read_line(&mut line).await.unwrap();
    assert_eq!(line, format!("CONNECT {target} HTTP/1.1\r\n"));
    loop {
        line.clear();
        assert_ne!(stream.read_line(&mut line).await.unwrap(), 0);
        if line == "\r\n" {
            break;
        }
    }
    stream
        .write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
        .await
        .unwrap();
    stream
}

async fn echo_connect(listener: &tokio::net::TcpListener, target: SocketAddr) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut stream = accept_connect(listener, target).await;
    let mut bytes = [0; DATA.len()];
    stream.read_exact(&mut bytes).await.unwrap();
    assert_eq!(bytes, DATA);
    stream.write_all(&bytes).await.unwrap();
}

async fn echo_dns(listener: &tokio::net::TcpListener, target: SocketAddr) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut stream = accept_connect(listener, target).await;
    let length = stream.read_u16().await.unwrap();
    assert_eq!(usize::from(length), DNS_QUERY.len());
    let mut query = vec![0; usize::from(length)];
    stream.read_exact(&mut query).await.unwrap();
    assert_eq!(query, DNS_QUERY);
    let reply = dns_answer();
    stream
        .write_u16(reply.len().try_into().unwrap())
        .await
        .unwrap();
    stream.write_all(&reply).await.unwrap();
}

fn dns_proxy_target() -> SocketAddr {
    SocketAddr::from((DNS_SERVER, DNS_PORT))
}

fn nonblocking(listener: TcpListener) -> TcpListener {
    listener.set_nonblocking(true).unwrap();
    listener
}

async fn verify_tcp_packet_flow(
    listener: &tokio::net::TcpListener,
    proxy: SocketAddr,
    target: SocketAddr,
    source: IpAddr,
) {
    tokio::time::timeout(Duration::from_secs(8), async {
        tokio::join!(
            echo_connect(listener, target),
            exercise_packet_device(proxy, target, source)
        );
    })
    .await
    .expect("packet TCP exchange completed before timeout");
}

async fn exercise_packet_device(proxy: SocketAddr, target: SocketAddr, source: IpAddr) {
    let (ingress, read) = mpsc::channel(32);
    let (write, mut egress) = mpsc::channel(32);
    let stop = CancellationToken::new();
    let relay = tokio::spawn(tun2proxy::run(
        PacketDevice { read, write },
        MTU,
        relay_options(proxy).unwrap(),
        stop.clone(),
    ));
    let mut syn = Vec::new();
    packet(1, target, source)
        .syn()
        .write(&mut syn, &[])
        .unwrap();
    ingress.send(syn).await.unwrap();
    let reply = egress.recv().await.unwrap();
    let tcp = tcp(&reply);
    assert!(tcp.syn() && tcp.ack());
    assert_eq!(tcp.acknowledgment_number(), 2);
    let mut data = Vec::new();
    packet(2, target, source)
        .ack(tcp.sequence_number().wrapping_add(1))
        .psh()
        .write(&mut data, DATA)
        .unwrap();
    ingress.send(data).await.unwrap();
    loop {
        let reply = egress.recv().await.unwrap();
        let tcp = self::tcp(&reply);
        if !tcp.payload().is_empty() {
            assert_eq!(tcp.payload(), DATA);
            assert_eq!(reply[0] >> 4, if source.is_ipv6() { 6 } else { 4 });
            break;
        }
    }
    stop.cancel();
    relay.await.unwrap().unwrap();
}

async fn exercise_udp_packet(
    proxy: SocketAddr,
    target: SocketAddrV4,
    payload: &[u8],
    response: &[u8],
) {
    let (ingress, read) = mpsc::channel(32);
    let (write, mut egress) = mpsc::channel(32);
    let stop = CancellationToken::new();
    let relay = tokio::spawn(tun2proxy::run(
        PacketDevice { read, write },
        MTU,
        relay_options(proxy).unwrap(),
        stop.clone(),
    ));
    let mut packet = Vec::new();
    PacketBuilder::ipv4(TUN_ADDRESS.octets(), target.ip().octets(), 64)
        .udp(51001, target.port())
        .write(&mut packet, payload)
        .unwrap();
    ingress.send(packet).await.unwrap();
    let reply = egress.recv().await.unwrap();
    let packet = SlicedPacket::from_ip(&reply).unwrap();
    let (Some(NetSlice::Ipv4(ip)), Some(TransportSlice::Udp(udp))) = (packet.net, packet.transport)
    else {
        panic!("expected IPv4 UDP response");
    };
    assert_eq!(ip.header().source(), target.ip().octets());
    assert_eq!(ip.header().destination(), TUN_ADDRESS.octets());
    assert_eq!(
        (udp.source_port(), udp.destination_port()),
        (target.port(), 51001)
    );
    assert_eq!(udp.payload(), response);
    stop.cancel();
    relay.await.unwrap().unwrap();
}

fn packet(
    sequence: u32,
    target: SocketAddr,
    source: IpAddr,
) -> etherparse::PacketBuilderStep<etherparse::TcpHeader> {
    let ip = match (source, target.ip()) {
        (IpAddr::V4(from), IpAddr::V4(to)) => PacketBuilder::ipv4(from.octets(), to.octets(), 64),
        (IpAddr::V6(from), IpAddr::V6(to)) => PacketBuilder::ipv6(from.octets(), to.octets(), 64),
        _ => panic!("fixture IP family differs"),
    };
    ip.tcp(51000, target.port(), sequence, 65535)
}

fn tcp(packet: &[u8]) -> etherparse::TcpSlice<'_> {
    let Some(TransportSlice::Tcp(tcp)) = SlicedPacket::from_ip(packet).unwrap().transport else {
        panic!("expected TCP packet");
    };
    tcp
}

struct PacketDevice {
    read: mpsc::Receiver<Vec<u8>>,
    write: mpsc::Sender<Vec<u8>>,
}

impl AsyncRead for PacketDevice {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.read.poll_recv(cx) {
            Poll::Ready(Some(packet)) if packet.len() <= buffer.remaining() => {
                buffer.put_slice(&packet);
                Poll::Ready(Ok(()))
            }
            Poll::Ready(None) => Poll::Ready(Ok(())),
            Poll::Ready(Some(_)) => Poll::Ready(Err(io::ErrorKind::InvalidData.into())),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl AsyncWrite for PacketDevice {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        packet: &[u8],
    ) -> Poll<io::Result<usize>> {
        Poll::Ready(
            self.write
                .try_send(packet.to_vec())
                .map(|()| packet.len())
                .map_err(io::Error::other),
        )
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}
