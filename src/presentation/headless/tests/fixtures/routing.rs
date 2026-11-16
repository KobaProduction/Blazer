use std::ffi::OsStr;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener, TcpStream};
use std::process::{Child, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use super::TempData;

const TIMEOUT: Duration = Duration::from_secs(8);
const DNS_NAME: &str = "direct.route.test";
const BLOCKED_NAME: &str = "blocked.route.test";
const PAYLOAD: &[u8] = b"headless routing\0";

#[test]
fn routing_commands_persist_order_assignment_and_dns() {
    let data = TempData::new();
    let first = b"block domain denied.test\ndirect suffix test\nproxy cidr 192.0.2.0/24\n";
    let uuid = add_profile(&data, first);
    assign_global(&data, &uuid);
    let listed = data.run(&["routing"], None);
    assert!(listed.contains(&format!("routing_global={uuid}")));

    let before = data.run(&["routing", "show", &uuid], None);
    assert!(before.contains("revision=1"));
    assert!(before.ends_with(std::str::from_utf8(first).unwrap()));

    data.run(
        &[
            "routing",
            "dns",
            &uuid,
            "https",
            "[::1]:443",
            "dns.test",
            "/dns-query",
        ],
        None,
    );
    let updated = data.run(&["routing", "show", &uuid], None);
    assert!(updated.contains("revision=2"));
    assert!(updated.contains("dns https [::1]:443 dns.test /dns-query"));
    assert!(updated.ends_with(std::str::from_utf8(first).unwrap()));

    let copied = data.run(&["routing", "copy", &uuid, "Copied rules"], None);
    let copy_uuid = saved_uuid(&copied);
    assert_ne!(copy_uuid, uuid);
    let copied = data.run(&["routing", "show", copy_uuid], None);
    assert!(copied.contains("dns https [::1]:443 dns.test /dns-query"));
    assert!(copied.ends_with(std::str::from_utf8(first).unwrap()));

    let rejected = data
        .command()
        .args(["routing", "remove", &uuid])
        .output()
        .unwrap();
    assert!(!rejected.status.success(), "referenced profile must remain");
    data.run(&["routing", "assign", "global", "none", "disabled"], None);
    data.run(&["routing", "remove", &uuid], None);
    assert!(!data.run(&["routing"], None).contains(&uuid));
}

#[test]
#[ignore = "requires VPN_CLIENT_TEST_SHOES_EXE pointing to the SHA-256-pinned Windows Shoes executable"]
fn routing_commands_drive_localproxy_dns_and_first_match() {
    let shoes = std::env::var_os("VPN_CLIENT_TEST_SHOES_EXE")
        .expect("set VPN_CLIENT_TEST_SHOES_EXE to the approved local Shoes binary");
    let shoes = std::fs::canonicalize(shoes).expect("approved Shoes path");
    let data = TempData::new();
    let direct_v4 = bind(Ipv4Addr::LOCALHOST);
    let direct_v6 = bind(Ipv6Addr::LOCALHOST);
    let upstream = bind(Ipv4Addr::LOCALHOST);
    let dns = bind(Ipv4Addr::LOCALHOST);
    let input = format!(
        "dns tcp {}\nblock domain {BLOCKED_NAME}\ndirect suffix route.test\ndirect cidr 127.0.0.1/32\ndirect cidr ::1/128\nproxy cidr 127.0.0.2/32\nblock cidr 127.0.0.2/32\n",
        dns.local_addr().unwrap()
    );
    let uuid = add_profile(&data, input.as_bytes());
    assign_global(&data, &uuid);
    let profile = format!(
        "http://{}#Loopback-route-fixture",
        upstream.local_addr().unwrap()
    );
    data.run(&["import"], Some(profile.as_bytes()));
    let (mut process, address, active) = start_proxy(&data, shoes.as_os_str());
    assert_eq!(active.trim(), format!("routing_active={uuid} revision=1"));

    assert_blocked(address, direct_v4.local_addr().unwrap().port());
    assert!(no_pending(&direct_v4));
    assert!(no_pending(&upstream));
    assert!(no_pending(&dns));

    assert_direct_routes(address, &direct_v4, &direct_v6, &dns);
    assert_proxy_route(address, &upstream, Ipv4Addr::new(127, 0, 0, 2));
    assert_proxy_route(address, &upstream, Ipv4Addr::new(127, 0, 0, 3));
    process.stop();
    TcpListener::bind(address).expect("headless shutdown released proxy port");
}

fn add_profile(data: &TempData, input: &[u8]) -> String {
    let output = data.run(&["routing", "add", "Rules fixture"], Some(input));
    saved_uuid(&output).to_owned()
}

fn saved_uuid(output: &str) -> &str {
    output
        .strip_prefix("routing_profile_saved=")
        .and_then(|line| line.split_whitespace().next())
        .expect("saved routing profile UUID")
}

fn assign_global(data: &TempData, uuid: &str) {
    data.run(&["routing", "assign", "global", uuid, "enabled"], None);
}

struct ProxyProcess(Child);

impl ProxyProcess {
    fn stop(&mut self) {
        drop(self.0.stdin.take());
        let deadline = Instant::now() + TIMEOUT;
        loop {
            if let Some(status) = self.0.try_wait().expect("headless status") {
                assert!(status.success(), "headless shutdown failed: {status}");
                return;
            }
            assert!(Instant::now() < deadline, "headless shutdown timed out");
            thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for ProxyProcess {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn start_proxy(data: &TempData, shoes: &OsStr) -> (ProxyProcess, SocketAddr, String) {
    let mut process = ProxyProcess(
        data.command()
            .arg("proxy")
            .env("VPN_CLIENT_SHOES_PATH", shoes)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("start headless LocalProxy"),
    );
    let stdout = BufReader::new(process.0.stdout.take().unwrap());
    let (ready, lines) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let _ = ready.send(stdout.lines().take(3).collect::<Result<Vec<_>, _>>());
    });
    let lines = lines
        .recv_timeout(TIMEOUT)
        .expect("headless startup timed out")
        .expect("read headless startup lines");
    let [proxy, http, active] = lines.as_slice() else {
        panic!("headless startup ended early: {lines:?}")
    };
    let address = proxy
        .strip_prefix("local_proxy=")
        .expect("headless reports its local proxy")
        .parse()
        .unwrap();
    assert!(http.starts_with("http_proxy="), "{http:?}");
    (process, address, active.clone())
}

fn bind(address: impl Into<std::net::IpAddr>) -> TcpListener {
    let listener =
        TcpListener::bind(SocketAddr::new(address.into(), 0)).expect("loopback listener");
    listener.set_nonblocking(true).unwrap();
    listener
}

fn accept(listener: &TcpListener) -> TcpStream {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        match listener.accept() {
            Ok((socket, _)) => {
                socket.set_nonblocking(false).unwrap();
                return with_timeout(socket);
            }
            Err(error)
                if error.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline =>
            {
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => panic!("accept loopback connection: {error}"),
        }
    }
}

fn no_pending(listener: &TcpListener) -> bool {
    listener
        .accept()
        .is_err_and(|error| error.kind() == std::io::ErrorKind::WouldBlock)
}

fn with_timeout(socket: TcpStream) -> TcpStream {
    socket.set_read_timeout(Some(TIMEOUT)).unwrap();
    socket.set_write_timeout(Some(TIMEOUT)).unwrap();
    socket
}

fn assert_direct_routes(proxy: SocketAddr, v4: &TcpListener, v6: &TcpListener, dns: &TcpListener) {
    let v4_address = v4.local_addr().unwrap();
    let v6_address = v6.local_addr().unwrap();
    thread::scope(|scope| {
        scope.spawn(|| serve_dns(dns));
        scope.spawn(|| serve_echo(v4, b"via-dns"));
        assert_echo(
            proxy,
            SocksTarget::Domain(DNS_NAME, v4_address.port()),
            b"via-dns",
        );
    });
    thread::scope(|scope| {
        scope.spawn(|| serve_echo(v4, b"via-v4"));
        assert_echo(proxy, SocksTarget::Ip(v4_address), b"via-v4");
    });
    thread::scope(|scope| {
        scope.spawn(|| serve_echo(v6, b"via-v6"));
        assert_echo(proxy, SocksTarget::Ip(v6_address), b"via-v6");
    });
}

fn assert_proxy_route(proxy: SocketAddr, upstream: &TcpListener, ip: Ipv4Addr) {
    let target = SocketAddr::new(ip.into(), 8443);
    thread::scope(|scope| {
        scope.spawn(|| {
            let mut request = BufReader::new(accept(upstream));
            let mut line = String::new();
            request.read_line(&mut line).unwrap();
            assert_eq!(line, format!("CONNECT {target} HTTP/1.1\r\n"));
            loop {
                line.clear();
                assert_ne!(request.read_line(&mut line).unwrap(), 0);
                if line == "\r\n" {
                    break;
                }
            }
            request
                .get_mut()
                .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                .unwrap();
            let mut payload = [0; PAYLOAD.len()];
            request.read_exact(&mut payload).unwrap();
            assert_eq!(payload, PAYLOAD);
            request.get_mut().write_all(b"via-upstream").unwrap();
        });
        assert_echo(proxy, SocksTarget::Ip(target), b"via-upstream");
    });
}

fn serve_echo(listener: &TcpListener, reply: &[u8]) {
    let mut socket = accept(listener);
    assert_eq!(read::<{ PAYLOAD.len() }>(&mut socket), PAYLOAD);
    socket.write_all(reply).unwrap();
}

fn serve_dns(listener: &TcpListener) {
    let mut socket = accept(listener);
    let length = u16::from_be_bytes(read::<2>(&mut socket));
    assert!((12..=512).contains(&length), "loopback DNS query size");
    let mut request = vec![0; usize::from(length)];
    socket.read_exact(&mut request).unwrap();
    let question = b"\x06direct\x05route\x04test\x00\x00\x01\x00\x01";
    assert_eq!(&request[4..6], &[0, 1], "one DNS question");
    assert_eq!(
        &request[12..12 + question.len()],
        question,
        "A query for routed hostname"
    );
    let mut response = Vec::from(&request[..2]);
    response.extend([0x81, 0x80, 0, 1, 0, 1, 0, 0, 0, 0]);
    response.extend(question);
    response.extend([0xc0, 0x0c, 0, 1, 0, 1, 0, 0, 0, 60, 0, 4, 127, 0, 0, 1]);
    socket
        .write_all(&u16::try_from(response.len()).unwrap().to_be_bytes())
        .unwrap();
    socket.write_all(&response).unwrap();
}

enum SocksTarget<'a> {
    Ip(SocketAddr),
    Domain(&'a str, u16),
}

fn socks_connect(proxy: SocketAddr, target: SocksTarget<'_>) -> TcpStream {
    let mut socket = with_timeout(TcpStream::connect_timeout(&proxy, TIMEOUT).unwrap());
    socket.write_all(&[5, 1, 0]).unwrap();
    assert_eq!(read::<2>(&mut socket), [5, 0]);
    let mut request = vec![5, 1, 0];
    match target {
        SocksTarget::Ip(SocketAddr::V4(address)) => {
            request.push(1);
            request.extend(address.ip().octets());
            request.extend(address.port().to_be_bytes());
        }
        SocksTarget::Ip(SocketAddr::V6(address)) => {
            request.push(4);
            request.extend(address.ip().octets());
            request.extend(address.port().to_be_bytes());
        }
        SocksTarget::Domain(name, port) => {
            request.extend([3, u8::try_from(name.len()).unwrap()]);
            request.extend(name.as_bytes());
            request.extend(port.to_be_bytes());
        }
    }
    socket.write_all(&request).unwrap();
    socket
}

fn assert_echo(proxy: SocketAddr, target: SocksTarget<'_>, expected: &[u8]) {
    let mut socket = socks_connect(proxy, target);
    let [version, status, reserved, address_type] = read::<4>(&mut socket);
    assert_eq!([version, status, reserved], [5, 0, 0]);
    match address_type {
        1 => _ = read::<6>(&mut socket),
        4 => _ = read::<18>(&mut socket),
        other => panic!("unexpected SOCKS address type {other}"),
    }
    socket.write_all(PAYLOAD).unwrap();
    let mut reply = vec![0; expected.len()];
    socket.read_exact(&mut reply).unwrap();
    assert_eq!(reply, expected);
}

fn assert_blocked(proxy: SocketAddr, port: u16) {
    let mut socket = socks_connect(proxy, SocksTarget::Domain(BLOCKED_NAME, port));
    let mut reply = [0; 2];
    match socket.read_exact(&mut reply) {
        Ok(()) => {
            assert_eq!(reply[0], 5);
            assert_ne!(
                reply[1], 0,
                "first Block rule must precede matching Direct suffix"
            );
        }
        Err(error) => assert!(
            matches!(
                error.kind(),
                std::io::ErrorKind::UnexpectedEof | std::io::ErrorKind::ConnectionReset
            ),
            "blocked socket must be rejected or closed: {error}"
        ),
    }
}

fn read<const N: usize>(socket: &mut TcpStream) -> [u8; N] {
    let mut bytes = [0; N];
    socket.read_exact(&mut bytes).unwrap();
    bytes
}
