use std::sync::Arc;

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use serde_json::{Value, json};
use vpn_application::client::{CancellationSignal, ClientNormalizerPort};
use vpn_domain::routing::{DnsPolicy, RouteAction, RouteRule, RouteTarget, RoutingInput};

use super::*;
use crate::client_normalize::SubscriptionNormalizer;

const ID: &str = "a5c41b77-fc31-4ab0-a275-1c67aa9aab31";
const HOST_HEADER: &str = "loopback.example.test";
const WS_PATH: &str = "/format-import";

#[test]
fn firefox_fingerprint_survives_tcp_import_and_engine_rendering() {
    for (security, parameters, fingerprint, expected) in [
        ("tls", "", "firefox", "firefox-148"),
        (
            "reality",
            "&pbk=KioqKioqKioqKioqKioqKioqKioqKioqKioqKioqKio&sid=abcd",
            "hellofirefox_120",
            "firefox-120",
        ),
    ] {
        let link = format!(
            "vless://{ID}@example.test:443?type=tcp&security={security}&sni=example.test&fp={fingerprint}{parameters}"
        );
        let profile = SubscriptionNormalizer
            .normalize(link.as_bytes())
            .unwrap()
            .remove(0);
        let config: Value =
            serde_json::from_slice(&config::render(&profile, 40000, None).unwrap()).unwrap();
        let wrapper = &config[0]["rules"][0]["client_chain"]["protocol"];
        assert_eq!(wrapper["type"], security);
        assert_eq!(wrapper["fingerprint"], expected);
    }
}

struct FixtureRoot(PathBuf);

impl FixtureRoot {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("vpn-import-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&path).expect("create unique fixture directory");
        Self(path)
    }
}

impl Drop for FixtureRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir(self.0.join("runtime"));
        let _ = fs::remove_dir(&self.0);
    }
}

struct OwnedChild(Child);

impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = stop_child(&mut self.0);
    }
}

fn trusted_binary() -> PathBuf {
    let path = std::env::var_os("BLAZER_TEST_BLAZECORE_EXE")
        .map(PathBuf::from)
        .expect("set BLAZER_TEST_BLAZECORE_EXE to the approved Windows BlazeCore executable");
    assert!(
        path.is_absolute(),
        "fixture must use an absolute engine path"
    );
    assert_eq!(file_sha256(&path), Ok(APPROVED_ENGINE_SHA256.to_owned()));
    path
}

#[test]
#[ignore = "requires the pinned Windows BlazeCore executable"]
fn approved_blazecore_validates_scoped_routing_and_dns_options() {
    let binary = trusted_binary();
    let temp = FixtureRoot::new();
    let private = private::PrivateDirectory::create(&temp.0).unwrap();
    let profile = SubscriptionNormalizer
        .normalize(b"socks5://127.0.0.1:1080#Routing")
        .unwrap()
        .remove(0);
    let mut routing = RoutingProfile {
        uuid: "e3005818-0152-4e86-ac8a-4ee982318128".into(),
        revision: 1,
        input: RoutingInput {
            title: "Policy".into(),
            rules: vec![
                RouteRule {
                    target: RouteTarget::Domain("blocked.example".into()),
                    action: RouteAction::Block,
                },
                RouteRule {
                    target: RouteTarget::DomainSuffix("example.net".into()),
                    action: RouteAction::Direct,
                },
                RouteRule {
                    target: RouteTarget::Cidr("192.0.2.0/24".into()),
                    action: RouteAction::Proxy,
                },
                RouteRule {
                    target: RouteTarget::Cidr("2001:db8::/32".into()),
                    action: RouteAction::Block,
                },
            ],
            dns: DnsPolicy::Automatic,
        },
    };
    let input = private.0.join("routing.yaml");
    for dns in [
        DnsPolicy::System,
        DnsPolicy::Tcp {
            endpoint: "192.0.2.53:53".parse().unwrap(),
        },
        DnsPolicy::Tls {
            endpoint: "[2001:db8::53]:853".parse().unwrap(),
            server_name: "dns.example".into(),
        },
        DnsPolicy::Https {
            endpoint: "[2001:db8::53]:443".parse().unwrap(),
            server_name: "dns.example".into(),
            path: "/dns-query".into(),
        },
    ] {
        routing.input.dns = dns;
        let bytes = config::render(&profile, 40_000, Some(&routing)).unwrap();
        fs::write(&input, bytes).unwrap();
        let mut check = engine_command(&binary, &private.0);
        check.arg("check").arg(&input);
        run_bounded(check, &cancellation(), ClientError::EngineInvalid).unwrap();
    }
}

fn cancellation() -> Cancellation {
    Cancellation::new(Arc::new(CancellationSignal::default()), 1)
}

#[test]
fn runtime_acl_excludes_parent_grants_from_the_directory_and_profile() {
    use win_custody::{
        ObjectSecurityDescriptor, OwnedSid, SecurityDescriptor, current_user_sid_string,
    };

    const SYSTEM_SID: &str = "S-1-5-18";
    const FILE_ALL_ACCESS: u32 = 0x001f_01ff;
    const OBJECT_AND_CONTAINER_INHERIT: u8 = 0x03;
    const INHERITED: u8 = 0x10;

    let root = FixtureRoot::new();
    let user_sid = current_user_sid_string().unwrap();
    let parent = format!("D:P(A;OICI;FA;;;{user_sid})(A;OICI;FA;;;SY)(A;OICI;FR;;;WD)");
    SecurityDescriptor::from_sddl(&parent)
        .unwrap()
        .apply_to_tree(&root.0, true)
        .unwrap();

    let runtime = private::PrivateDirectory::create(&root.0).unwrap();
    let profile = runtime.0.join("profile.yaml");
    fs::write(&profile, "synthetic fixture").unwrap();
    let user = OwnedSid::from_string(&user_sid).unwrap();
    let system = OwnedSid::from_string(SYSTEM_SID).unwrap();
    let expected_count = if user_sid == SYSTEM_SID { 1 } else { 2 };
    for (path, flags) in [
        (&runtime.0, OBJECT_AND_CONTAINER_INHERIT),
        (&profile, INHERITED),
    ] {
        let descriptor = ObjectSecurityDescriptor::query_file(path).unwrap();
        let acl = descriptor.dacl().unwrap();
        assert_eq!(acl.len(), expected_count);
        assert!(acl.allowed_aces().any(|ace| ace.trustee_matches(&user)));
        assert!(acl.allowed_aces().any(|ace| ace.trustee_matches(&system)));
        assert!(acl.allowed_aces().all(|ace| {
            ace.mask() == FILE_ALL_ACCESS
                && ace.flags() & (OBJECT_AND_CONTAINER_INHERIT | INHERITED) == flags
        }));
    }
}

#[test]
fn timed_process_wait_remains_cancelable() {
    let ping = PathBuf::from(std::env::var_os("SystemRoot").expect("Windows system directory"))
        .join("System32")
        .join("PING.EXE");
    let directory = FixtureRoot::new();
    let mut child = OwnedChild(
        engine_command(&ping, &directory.0)
            .args(["-n", "15", "127.0.0.1"])
            .spawn()
            .expect("start hidden loopback fixture"),
    );
    let signal = Arc::new(CancellationSignal::default());
    let cancel = Cancellation::new(signal.clone(), 1);
    let started = Instant::now();
    let result = std::thread::scope(|scope| {
        scope.spawn(|| {
            std::thread::sleep(Duration::from_millis(80));
            signal.cancel(1);
        });
        wait_child(&mut child.0, &cancel, ClientError::EngineInvalid)
    });
    assert_eq!(result, Err(ClientError::Cancelled));
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(child.0.try_wait().unwrap().is_some());
}

fn check(binary: &Path, directory: &Path, file: &Path) {
    let mut command = engine_command(binary, directory);
    command.arg("check").arg(file);
    run_bounded(command, &cancellation(), ClientError::EngineInvalid)
        .expect("pinned BlazeCore validates rendered fixture");
}

#[test]
#[ignore = "requires the pinned Windows BlazeCore binary via BLAZER_TEST_BLAZECORE_EXE"]
fn approved_blazecore_checks_imported_protocol_and_transport_mappings() {
    let binary = trusted_binary();
    let key = URL_SAFE_NO_PAD.encode([42; 32]);
    let ss_128 = STANDARD.encode([17; 16]);
    let ss_256 = STANDARD.encode([23; 32]);
    let input = json!([
        "socks5://fixture-user:fixture-password@[::1]:1080",
        "https://fixture-user:fixture-password@127.0.0.1:18443?sni=localhost&alpn=http%2F1.1",
        format!("vless://{ID}@127.0.0.1:1443?type=raw"),
        {"protocol":"vless","settings":{"address":"localhost","port":2443,"id":ID,
         "flow":"xtls-rprx-vision","encryption":"none"},"streamSettings":{
         "method":"raw","rawSettings":{"header":{"type":"none"}},"security":"tls",
         "tlsSettings":{"serverName":"localhost"}}},
        {"type":"vless","server":"localhost","server_port":3443,"uuid":ID,
         "flow":"xtls-rprx-vision","tls":{"enabled":true,"server_name":"localhost",
         "reality":{"enabled":true,"public_key":key,"short_id":"abcd"}}},
        {"protocol":"vmess","settings":{"address":"localhost","port":4443,"id":ID},
         "streamSettings":{"method":"websocket","wsSettings":{"path":WS_PATH,"host":HOST_HEADER},
         "security":"tls","tlsSettings":{"serverName":"localhost"}}},
        {"type":"trojan","server":"localhost","server_port":5443,"password":"fixture-password",
         "tls":{"enabled":true,"server_name":"localhost"}},
        format!("vless://{ID}@localhost:6443?type=xhttp&security=tls&alpn=h3&path=%2Fproxy&mode=stream-one"),
        format!("vless://{ID}@localhost:6444?type=xhttp&security=reality&pbk={key}&sni=localhost&mode=auto"),
        "ss://aes-128-gcm:fixture-password@localhost:6443",
        {"type":"shadowsocks","server":"localhost","server_port":7443,
         "method":"2022-blake3-aes-128-gcm","password":format!("{ss_128}:{ss_128}")},
        {"type":"shadowsocks","server":"localhost","server_port":8443,
         "method":"2022-blake3-chacha20-poly1305","password":ss_256}
    ]);
    let profiles = SubscriptionNormalizer
        .normalize(&serde_json::to_vec(&input).unwrap())
        .expect("normalize mapped protocol/transport fixtures");
    assert_eq!(profiles.len(), 12);
    let servers = profiles
        .iter()
        .zip(40_000..)
        .map(|(profile, port)| {
            let bytes = config::render(profile, port, None).expect("render normalized profile");
            let [value]: [Value; 1] = serde_json::from_slice(&bytes).expect("one server");
            value
        })
        .collect::<Vec<_>>();
    let root = FixtureRoot::new();
    let directory = private::PrivateDirectory::create(&root.0).expect("private check directory");
    let file = directory.0.join("profile.yaml");
    fs::write(&file, serde_json::to_vec(&servers).unwrap()).expect("write synthetic configuration");
    check(&binary, &directory.0, &file);
}

#[test]
#[ignore = "requires the pinned Windows BlazeCore binary via BLAZER_TEST_BLAZECORE_EXE"]
fn xray_xhttp_modes_carry_exact_loopback_payload() {
    let binary = trusted_binary();
    let root = FixtureRoot::new();
    let reservation = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let upstream = reservation.local_addr().unwrap();
    let directory = private::PrivateDirectory::create(&root.0).unwrap();
    let _server = start_loopback_server(
        &binary,
        &directory.0,
        reservation,
        json!({
            "type":"xhttp", "path":"/split", "host":HOST_HEADER, "mode":"auto",
            "protocol":{"type":"vless", "user_id":ID}
        }),
    );
    let mut proxy = BlazeCoreProcess::new(
        TrustedEngineSpec {
            source_path: binary,
            expected_sha256: APPROVED_ENGINE_SHA256.into(),
            expected_version: APPROVED_ENGINE_VERSION.into(),
        },
        root.0.clone(),
        0,
    );
    for mode in ["auto", "packet-up", "stream-up", "stream-one"] {
        let outbound = json!({
            "protocol":"vless", "settings":{"id":ID,"address":"127.0.0.1","port":upstream.port()},
            "streamSettings":{"network":"xhttp","xhttpSettings":{"path":"/split?fixture=1","host":HOST_HEADER,"mode":mode}}
        });
        let profile = SubscriptionNormalizer
            .normalize(&serde_json::to_vec(&outbound).unwrap())
            .unwrap()
            .remove(0);
        proxy
            .start(ConnectionMode::LocalProxy, &profile, 0, &cancellation())
            .expect(mode);
        let EngineRuntime::Running(RunningConnection::LocalProxy(local)) = proxy.poll().unwrap()
        else {
            panic!("XHTTP proxy did not publish its endpoint in mode {mode}");
        };
        proxy_echo(local);
        proxy.stop().unwrap();
    }
}

#[test]
#[ignore = "requires the pinned Windows BlazeCore binary via BLAZER_TEST_BLAZECORE_EXE"]
fn imported_xray_xhttp_packet_up_uses_configured_sequence_metadata() {
    const EXCHANGES: &[EchoExchange] = &[
        (b"\x00first\xff", b"\xffresponse-one\x00"),
        (b"second\x00\xff", b"response-two\xff\x00"),
    ];
    let binary = trusted_binary();
    let root = FixtureRoot::new();
    let directory = private::PrivateDirectory::create(&root.0).unwrap();
    let mut proxy = BlazeCoreProcess::new(
        TrustedEngineSpec {
            source_path: binary.clone(),
            expected_sha256: APPROVED_ENGINE_SHA256.into(),
            expected_version: APPROVED_ENGINE_VERSION.into(),
        },
        root.0.clone(),
        0,
    );
    for (session, sequence) in [
        ("header", "query"),
        ("cookie", "header"),
        ("query", "cookie"),
    ] {
        let reservation = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = reservation.local_addr().unwrap().port();
        let metadata = json!({
            "sessionIDPlacement":session, "sessionIDKey":"blazer-test-session",
            "seqPlacement":sequence, "seqKey":"blazer-test-sequence",
        });
        // The strict packet-up server accepts uploads only with both configured metadata fields.
        let mut inbound = metadata.clone();
        inbound["type"] = json!("xhttp");
        inbound["path"] = json!("/sequence");
        inbound["host"] = json!(HOST_HEADER);
        inbound["mode"] = json!("packet-up");
        inbound["protocol"] = json!({"type":"vless", "user_id":ID});
        let _server = start_loopback_server(&binary, &directory.0, reservation, inbound);
        let input = json!({
            "outbounds":[{
                "protocol":"vless",
                "settings":{"vnext":[{"address":"127.0.0.1","port":port,"users":[{"id":ID,"encryption":"none"}]}]},
                "streamSettings":{
                    "network":"xhttp",
                    "xhttpSettings":{"path":"/sequence?fixture=1","host":HOST_HEADER,"mode":"packet-up","extra":metadata}
                }
            }]
        });
        let profile = SubscriptionNormalizer
            .normalize(&serde_json::to_vec(&input).unwrap())
            .expect("import Xray XHTTP packet-up profile")
            .remove(0);
        proxy
            .start(ConnectionMode::LocalProxy, &profile, 0, &cancellation())
            .expect("start imported XHTTP profile");
        let EngineRuntime::Running(RunningConnection::LocalProxy(local)) = proxy.poll().unwrap()
        else {
            panic!("missing XHTTP proxy endpoint for session={session}, sequence={sequence}");
        };
        // Waiting for the first response before writing again exercises multiple upload sequences.
        proxy_echo_exchanges(local, EXCHANGES);
        proxy.stop().unwrap();
    }
}

#[test]
#[ignore = "requires the pinned Windows BlazeCore binary via BLAZER_TEST_BLAZECORE_EXE"]
fn xray_vless_encryption_carries_exact_loopback_payload() {
    let binary = trusted_binary();
    encrypted_vless_round_trip(&binary, false, "native", "0rtt");
    encrypted_vless_round_trip(&binary, true, "xorpub", "1rtt");
}

fn encrypted_vless_round_trip(binary: &Path, vision: bool, appearance: &str, rtt: &str) {
    let root = FixtureRoot::new();
    let directory = private::PrivateDirectory::create(&root.0).unwrap();
    let pair = generate_vless_pair(binary, &directory.0, appearance, rtt);
    let setting = |prefix| {
        pair.lines()
            .find_map(|line| line.strip_prefix(prefix))
            .expect("generator returned both VLESS settings")
    };
    let reservation = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = reservation.local_addr().unwrap().port();
    let protocol = json!({
        "type":"vless","user_id":ID,"decryption":setting("decryption: "),"vision":vision
    });
    let (protocol, stream) = if vision {
        (
            json!({"type":"ws","targets":[{"matching_path":WS_PATH,"protocol":protocol}]}),
            json!({"network":"ws","wsSettings":{"path":WS_PATH}}),
        )
    } else {
        (protocol, json!({"network":"raw"}))
    };
    let _upstream = start_loopback_server(binary, &directory.0, reservation, protocol);
    let outbound = json!({
        "protocol":"vless","settings":{
            "id":ID,"address":"127.0.0.1","port":port,"encryption":setting("encryption: "),
            "flow":if vision {"xtls-rprx-vision"} else {""}
        },"streamSettings":stream
    });
    let profile = SubscriptionNormalizer
        .normalize(&serde_json::to_vec(&outbound).unwrap())
        .unwrap()
        .remove(0);
    let mut proxy = BlazeCoreProcess::new(
        TrustedEngineSpec {
            source_path: binary.to_owned(),
            expected_sha256: APPROVED_ENGINE_SHA256.into(),
            expected_version: APPROVED_ENGINE_VERSION.into(),
        },
        root.0.clone(),
        0,
    );
    proxy
        .start(ConnectionMode::LocalProxy, &profile, 0, &cancellation())
        .expect("start native VLESS proxy");
    let EngineRuntime::Running(RunningConnection::LocalProxy(local)) = proxy.poll().unwrap() else {
        panic!("native VLESS proxy endpoint missing");
    };
    proxy_echo(local);
    proxy.stop().unwrap();
}

fn generate_vless_pair(
    binary: &Path,
    directory: &Path,
    appearance: &str,
    rtt: &str,
) -> Zeroizing<String> {
    let lifetime = if rtt == "1rtt" { "0s" } else { "600s" };
    let mut child = engine_command(binary, directory)
        .args([
            "generate-vless-encryption",
            "--auth",
            "x25519",
            "--mode",
            appearance,
            "--rtt",
            rtt,
            "--lifetime",
            lifetime,
        ])
        .stdout(Stdio::piped())
        .spawn()
        .expect("start the approved VLESS generator");
    wait_child(&mut child, &cancellation(), ClientError::EngineInvalid).unwrap();
    let mut output = Zeroizing::new(String::new());
    child
        .stdout
        .take()
        .unwrap()
        .take(4096)
        .read_to_string(&mut output)
        .unwrap();
    output
}

#[test]
#[ignore = "requires the pinned Windows BlazeCore binary via BLAZER_TEST_BLAZECORE_EXE"]
fn sing_box_vmess_websocket_carries_exact_loopback_payload() {
    let binary = trusted_binary();
    let root = FixtureRoot::new();
    let reservation = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("reserve fixture server");
    let upstream = reservation.local_addr().unwrap();
    let directory = private::PrivateDirectory::create(&root.0).expect("private fixture directory");
    let mut server = start_ws_server(&binary, &directory.0, reservation);

    let input = json!({
        "outbounds":[{
            "type":"vmess","server":"127.0.0.1","server_port":upstream.port(),
            "uuid":ID,"security":"aes-128-gcm","alter_id":0,
            "transport":{"type":"ws","path":WS_PATH,"headers":{"Host":HOST_HEADER}}
        }]
    });
    let profile = SubscriptionNormalizer
        .normalize(&serde_json::to_vec(&input).unwrap())
        .expect("normalize sing-box VMess WebSocket")
        .remove(0);
    let mut proxy = BlazeCoreProcess::new(
        TrustedEngineSpec {
            source_path: binary,
            expected_sha256: APPROVED_ENGINE_SHA256.to_owned(),
            expected_version: APPROVED_ENGINE_VERSION.to_owned(),
        },
        root.0.clone(),
        0,
    );
    proxy
        .start(ConnectionMode::LocalProxy, &profile, 0, &cancellation())
        .expect("start rendered profile");
    let EngineRuntime::Running(RunningConnection::LocalProxy(local)) =
        proxy.poll().expect("read local proxy runtime")
    else {
        panic!("local proxy did not report its bound address");
    };
    proxy_echo(local);
    proxy.stop().expect("stop owned local proxy");
    let _released = TcpListener::bind(local).expect("local proxy port released");
    stop_child(&mut server.0).expect("stop owned upstream");
    let _released = TcpListener::bind(upstream).expect("upstream port released");
}

fn start_ws_server(binary: &Path, directory: &Path, reservation: TcpListener) -> OwnedChild {
    start_loopback_server(
        binary,
        directory,
        reservation,
        json!({"type":"ws","targets":[
            {"matching_path":WS_PATH,"matching_headers":{"Host":HOST_HEADER},
             "protocol":{"type":"vmess","user_id":ID,"cipher":"aes-128-gcm"}}
        ]}),
    )
}

#[test]
#[ignore = "requires the pinned Windows BlazeCore binary via BLAZER_TEST_BLAZECORE_EXE"]
fn imported_websocket_early_data_reaches_the_peer_before_websocket_frames() {
    let binary = trusted_binary();
    let root = FixtureRoot::new();
    let directory = private::PrivateDirectory::create(&root.0).unwrap();
    let mut proxy = BlazeCoreProcess::new(
        TrustedEngineSpec {
            source_path: binary.clone(),
            expected_sha256: APPROVED_ENGINE_SHA256.into(),
            expected_version: APPROVED_ENGINE_VERSION.into(),
        },
        root.0.clone(),
        0,
    );
    for carrier in [Some("Sec-WebSocket-Protocol"), Some("X-Start"), None] {
        let reservation = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let upstream = reservation.local_addr().unwrap();
        let mut server_target = json!({
            "matching_path":WS_PATH,"matching_headers":{"Host":HOST_HEADER},
            "max_early_data":2560,"protocol":{"type":"vless","user_id":ID}
        });
        if let Some(carrier) = carrier {
            server_target["early_data_header_name"] = json!(carrier);
        }
        let _server = start_loopback_server(
            &binary,
            &directory.0,
            reservation,
            json!({"type":"ws","targets":[server_target]}),
        );
        let relay = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let endpoint = relay.local_addr().unwrap();
        let observer = std::thread::spawn(move || observe_ws_upgrade(relay, upstream, carrier));
        let input = early_ws_subscription(endpoint, carrier);
        let profile = SubscriptionNormalizer
            .normalize(input.as_bytes())
            .unwrap()
            .remove(0);
        proxy
            .start(ConnectionMode::LocalProxy, &profile, 0, &cancellation())
            .unwrap();
        let EngineRuntime::Running(RunningConnection::LocalProxy(local)) = proxy.poll().unwrap()
        else {
            panic!("WebSocket proxy did not publish its endpoint");
        };
        proxy_echo(local);
        proxy.stop().unwrap();
        observer.join().unwrap();
    }
}

fn early_ws_subscription(endpoint: SocketAddr, carrier: Option<&str>) -> String {
    if carrier == Some("Sec-WebSocket-Protocol") {
        return format!(
            "vless://{ID}@{endpoint}?type=ws&host={HOST_HEADER}&path=%2Fformat-import%3Fed%3D2560"
        );
    }
    let config = json!({"type":"vless","server":"127.0.0.1","server_port":endpoint.port(),"uuid":ID,
        "transport":{"type":"ws","path":WS_PATH,"headers":{"Host":HOST_HEADER},
            "max_early_data":2560,"early_data_header_name":carrier.unwrap_or("")}});
    URL_SAFE_NO_PAD.encode(config.to_string())
}

fn observe_ws_upgrade(relay: TcpListener, upstream: SocketAddr, carrier: Option<&str>) {
    let (mut client, _) = relay.accept().unwrap();
    tune_stream(&client);
    let mut request = Vec::new();
    while !request.ends_with(b"\r\n\r\n") {
        assert!(request.len() < 8192, "bounded WebSocket fixture handshake");
        let mut byte = [0];
        client.read_exact(&mut byte).unwrap();
        request.push(byte[0]);
    }
    let request = std::str::from_utf8(&request).unwrap();
    let encoded = if let Some(carrier) = carrier {
        assert!(request.starts_with(&format!("GET {WS_PATH} HTTP/1.1\r\n")));
        request
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case(carrier).then_some(value.trim())
            })
            .expect("early data in the configured header")
    } else {
        request
            .lines()
            .next()
            .unwrap()
            .strip_prefix(&format!("GET {WS_PATH}"))
            .unwrap()
            .strip_suffix(" HTTP/1.1")
            .unwrap()
    };
    let early = URL_SAFE_NO_PAD.decode(encoded).unwrap();
    assert_eq!(
        early.first(),
        Some(&0),
        "VLESS version is in the HTTP upgrade"
    );
    assert_eq!(&early[1..17], uuid::Uuid::parse_str(ID).unwrap().as_bytes());

    let mut server = TcpStream::connect_timeout(&upstream, Duration::from_secs(3)).unwrap();
    tune_stream(&server);
    server.write_all(request.as_bytes()).unwrap();
    let mut client_read = client.try_clone().unwrap();
    let mut server_read = server.try_clone().unwrap();
    std::thread::scope(|scope| {
        scope.spawn(|| {
            let _ = io::copy(&mut client_read, &mut server);
            let _ = server.shutdown(std::net::Shutdown::Write);
        });
        let _ = io::copy(&mut server_read, &mut client);
        let _ = client.shutdown(std::net::Shutdown::Write);
    });
}

fn start_loopback_server(
    binary: &Path,
    directory: &Path,
    reservation: TcpListener,
    protocol: Value,
) -> OwnedChild {
    let file = directory.join("profile.yaml");
    let server =
        json!([{"address": reservation.local_addr().unwrap().to_string(), "protocol": protocol}]);
    fs::write(&file, serde_json::to_vec(&server).unwrap())
        .expect("write protocol upstream fixture");
    check(binary, directory, &file);
    let mut command = engine_command(binary, directory);
    command
        .args(["--threads", "1", "--no-reload"])
        .arg(&file)
        .stdout(Stdio::piped());
    drop(reservation);
    let mut child = OwnedChild(command.spawn().expect("start approved upstream"));
    let marker = watch_readiness(child.0.stdout.take().expect("private readiness pipe")).unwrap();
    marker
        .recv_timeout(Duration::from_secs(8))
        .expect("owned upstream ready");
    child
}

type EchoExchange = (&'static [u8], &'static [u8]);

fn proxy_echo(local: SocketAddr) {
    const REQUEST: &[u8; 8] = b"\x00\x98\xbb\x02\x03\x04\xa3\xff";
    const RESPONSE: &[u8; 8] = b"\xff\xa3\x04\x03\x02\xbb\x98\x00";
    proxy_echo_exchanges(local, &[(REQUEST, RESPONSE)]);
}

fn proxy_echo_exchanges(local: SocketAddr, exchanges: &'static [EchoExchange]) {
    let target = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("own echo target");
    target.set_nonblocking(true).unwrap();
    let port = target.local_addr().unwrap().port().to_be_bytes();
    let echo = std::thread::spawn(move || {
        let limit = Instant::now() + Duration::from_secs(8);
        loop {
            match target.accept() {
                Ok((mut peer, _)) => {
                    peer.set_nonblocking(false).unwrap();
                    tune_stream(&peer);
                    for &(request, response) in exchanges {
                        let mut received = vec![0; request.len()];
                        peer.read_exact(&mut received).unwrap();
                        assert_eq!(received, request);
                        peer.write_all(response).unwrap();
                    }
                    break;
                }
                Err(error)
                    if error.kind() == io::ErrorKind::WouldBlock && Instant::now() < limit =>
                {
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(error) => panic!("local echo was not reached: {error}"),
            }
        }
    });
    let mut proxy = TcpStream::connect_timeout(&local, Duration::from_secs(3)).unwrap();
    tune_stream(&proxy);
    proxy.write_all(&[5, 1, 0]).unwrap();
    let mut method = [0; 2];
    proxy.read_exact(&mut method).unwrap();
    assert_eq!(method, [5, 0]);
    proxy
        .write_all(&[5, 1, 0, 1, 127, 0, 0, 1, port[0], port[1]])
        .unwrap();
    let mut reply = [0; 10];
    proxy.read_exact(&mut reply).unwrap();
    assert_eq!(&reply[..4], &[5, 0, 0, 1]);
    for &(request, response) in exchanges {
        proxy.write_all(request).unwrap();
        let mut received = vec![0; response.len()];
        proxy.read_exact(&mut received).unwrap();
        assert_eq!(received, response);
    }
    echo.join().unwrap();
}

fn tune_stream(stream: &TcpStream) {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .unwrap();
}
