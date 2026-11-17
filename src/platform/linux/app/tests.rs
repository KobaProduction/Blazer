use super::*;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::time::{Duration, Instant};
use vpn_presentation::session::{ClientCommand, ClientSnapshot, ConnectionMode};

const PAYLOAD: &[u8] = b"blazer Linux loopback\0";
const PRIVATE_LABEL: &str = "Encrypted Linux profile fixture";

#[test]
#[ignore = "requires isolated XDG and an unlocked Secret Service plus a pinned sidecar"]
fn linux_catalog_reopens_and_local_proxy_relays_bytes() {
    assert!(
        std::env::var_os("BLAZER_TEST_KEYRING_DATA").is_some(),
        "run under the isolated Linux Secret Service fixture"
    );
    let directory = data_directory().unwrap();
    assert!(!directory.join("catalog.sealed").exists());
    let upstream = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let uri = format!(
        "http://{}#{}",
        upstream.local_addr().unwrap(),
        PRIVATE_LABEL.replace(' ', "%20")
    );

    let mut session = open_session(directory.clone()).unwrap();
    let imported = run(
        &session,
        ClientCommand::ImportText {
            title: PRIVATE_LABEL.to_owned(),
            text: uri,
        },
    );
    assert_eq!(imported.profiles.len(), 1);
    let selected = imported.profiles[0].profile_uuid.clone();
    run(
        &session,
        ClientCommand::SelectProfile {
            profile_uuid: selected.clone(),
        },
    );
    session.shutdown().unwrap();
    assert!(
        !fs::read(directory.join("catalog.sealed"))
            .unwrap()
            .windows(PRIVATE_LABEL.len())
            .any(|window| window == PRIVATE_LABEL.as_bytes())
    );

    let mut reopened = open_session(directory.clone()).unwrap();
    let connected = run(
        &reopened,
        ClientCommand::Connect {
            mode: ConnectionMode::LocalProxy,
        },
    );
    assert_eq!(
        connected.active_profile_uuid.as_deref(),
        Some(selected.as_str())
    );
    let local: SocketAddr = connected.local_proxy.unwrap().parse().unwrap();
    let echo = std::thread::spawn(move || serve_upstream(upstream));
    check_relay(local);
    echo.join().unwrap();
    run(&reopened, ClientCommand::Disconnect);
    reopened.shutdown().unwrap();
    let error = TcpStream::connect_timeout(&local, Duration::from_secs(2)).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::ConnectionRefused);
    assert!(
        fs::read_dir(directory.join("runtime"))
            .unwrap()
            .next()
            .is_none()
    );
}

fn run(session: &ClientHandle, command: ClientCommand) -> ClientSnapshot {
    let request = session.send(command).unwrap();
    let deadline = Instant::now() + Duration::from_secs(12);
    while Instant::now() < deadline {
        let snapshot = session.snapshot();
        if snapshot.completed_request == Some(request) {
            assert_eq!(snapshot.last_error, None);
            return snapshot;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("client command did not complete");
}

fn configure_timeout(stream: &TcpStream) {
    let timeout = Some(Duration::from_secs(3));
    stream.set_read_timeout(timeout).unwrap();
    stream.set_write_timeout(timeout).unwrap();
}

fn serve_upstream(listener: TcpListener) {
    let (mut stream, _) = listener.accept().unwrap();
    configure_timeout(&stream);
    let mut reader = BufReader::new(&mut stream);
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    assert!(line.starts_with("CONNECT 127.0.0.1:12345 "));
    loop {
        line.clear();
        assert_ne!(reader.read_line(&mut line).unwrap(), 0);
        if line == "\r\n" {
            break;
        }
    }
    let mut request = [0; PAYLOAD.len()];
    reader
        .get_mut()
        .write_all(b"HTTP/1.1 200 OK\r\n\r\n")
        .unwrap();
    reader.read_exact(&mut request).unwrap();
    assert_eq!(request, PAYLOAD);
    reader.get_mut().write_all(&request).unwrap();
}

fn check_relay(local: SocketAddr) {
    let mut stream = TcpStream::connect_timeout(&local, Duration::from_secs(2)).unwrap();
    configure_timeout(&stream);
    stream.write_all(&[5, 1, 0]).unwrap();
    let mut greeting = [0; 2];
    stream.read_exact(&mut greeting).unwrap();
    assert_eq!(greeting, [5, 0]);
    stream
        .write_all(&[5, 1, 0, 1, 127, 0, 0, 1, 48, 57])
        .unwrap();
    let mut established = [0; 10];
    stream.read_exact(&mut established).unwrap();
    assert_eq!(&established[..4], &[5, 0, 0, 1]);
    stream.write_all(PAYLOAD).unwrap();
    let mut echo = [0; PAYLOAD.len()];
    stream.read_exact(&mut echo).unwrap();
    assert_eq!(echo, PAYLOAD);
}
