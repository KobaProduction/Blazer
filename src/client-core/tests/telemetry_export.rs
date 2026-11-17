#![cfg(windows)]

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use client_core::{
    ClientCommand, ClientCoreOptions, ClientError, ClientHandle, ClientSnapshot,
    TelemetryBearerChange, TelemetryMode, TelemetrySettingsSummary,
};

const SOCKET_TIMEOUT: Duration = Duration::from_secs(5);
const TEST_TIMEOUT: Duration = Duration::from_secs(30);
const SUBSCRIPTION_TOKEN: &str = "synthetic-subscription-private-value";
const SUBSCRIPTION_TITLE: &str = "synthetic-private-subscription-title";
const PROFILE_TOKEN: &str = "synthetic-private-profile-password";
const PROFILE_TITLE: &str = "synthetic-private-profile-title";
const OTLP_BEARER: &str = "synthetic-private-otlp-bearer";
const OPERATOR_BEARER: &str = "synthetic-operator-otlp-bearer";
const CHILD_DATA: &str = "BLAZER_TELEMETRY_TEST_DATA";
const CHILD_PROVIDER: &str = "BLAZER_TELEMETRY_TEST_PROVIDER";
const OTLP_CONTENT_TYPE: &str = "application/x-protobuf";

#[test]
fn exports_three_otlp_signals_without_subscription_secrets() {
    let data = TestDirectory::new();
    let listener =
        TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind OTLP loopback collector");
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let mut child = start_client(address, &data);
    let capture = collect(&listener, &mut child.0);

    assert!(capture.served_subscription);
    for path in ["/manual/v1/logs", "/other/v1/logs", "/reject/v1/logs"] {
        assert!(capture.verifications.contains(path));
    }
    for prefix in ["/operator", "/manual"] {
        check_exports(&capture, prefix);
    }
}

fn check_exports(capture: &Capture, prefix: &str) {
    for (signal, expected) in [
        ("traces", ["client.subscription", "result"]),
        (
            "metrics",
            ["blazer.client.events", "blazer.client.operation.duration"],
        ),
        ("logs", ["client.subscription", "updated"]),
    ] {
        let route = format!("{prefix}/v1/{signal}");
        let payload = capture.exports.get(&route).expect(&route);
        expect_bytes(payload, "service.name");
        expect_bytes(payload, "blazer");
        for value in expected {
            expect_bytes(payload, value);
        }
        for secret in [
            SUBSCRIPTION_TOKEN,
            SUBSCRIPTION_TITLE,
            PROFILE_TOKEN,
            PROFILE_TITLE,
            OTLP_BEARER,
            OPERATOR_BEARER,
        ] {
            assert!(
                !contains_bytes(payload, secret),
                "{signal} contains private fixture data"
            );
        }
        if let Some(device_id) = &capture.device_id {
            assert!(
                !contains_bytes(payload, device_id),
                "{signal} contains the installation ID"
            );
        }
    }
}

fn start_client(address: SocketAddr, data: &TestDirectory) -> ClientProcess {
    let mut command = Command::new(std::env::current_exe().expect("resolve test executable"));
    for (key, _) in std::env::vars_os() {
        let name = key.to_string_lossy().to_ascii_uppercase();
        if name.starts_with("OTEL_")
            || ["HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY"].contains(&name.as_str())
        {
            command.env_remove(key);
        }
    }
    let child = command
        .args([
            "--exact",
            "run_telemetry_workload",
            "--ignored",
            "--nocapture",
        ])
        .env(CHILD_DATA, &data.0)
        .env(CHILD_PROVIDER, address.to_string())
        .env("NO_PROXY", "127.0.0.1")
        .env("OTEL_ENDPOINT", format!("http://{address}/operator"))
        .env("OTEL_BEARER_TOKEN", OPERATOR_BEARER)
        .env(
            "OTEL_EXPORTER_OTLP_ENDPOINT",
            format!("http://{address}/sdk"),
        )
        .env("OTEL_EXPORTER_OTLP_HEADERS", "x-sdk-fixture=inherited")
        .env("OTEL_EXPORTER_OTLP_PROTOCOL", "http/protobuf")
        .spawn()
        .expect("spawn backend in isolated test process");
    ClientProcess(child)
}

#[test]
#[ignore = "run only by the loopback OTLP integration test"]
fn run_telemetry_workload() {
    let data = PathBuf::from(std::env::var_os(CHILD_DATA).expect("isolated test directory"));
    let address: SocketAddr = std::env::var(CHILD_PROVIDER).unwrap().parse().unwrap();
    assert!(address.ip().is_loopback());
    let options = ClientCoreOptions {
        engine_path: data.join("absent-blazecore.exe"),
        data_dir: data.clone(),
        ..ClientCoreOptions::default()
    };
    let (mut backend, changed, started) = open_client(options.clone());
    assert!(started.telemetry_export_configured);
    assert_eq!(
        started.telemetry_settings,
        TelemetrySettingsSummary::default()
    );
    let imported = run_command(
        &backend,
        &changed,
        ClientCommand::AddSubscription {
            title: SUBSCRIPTION_TITLE.into(),
            url: format!("http://{address}/subscription?key={SUBSCRIPTION_TOKEN}"),
        },
    );
    assert_eq!(imported.subscriptions.len(), 1);
    assert_eq!(imported.profiles.len(), 1);
    let uuid = imported.subscriptions[0].subscription_uuid.clone();
    backend.shutdown().expect("flush the default collector");

    let (mut backend, changed, reopened_default) = open_client(options.clone());
    assert!(reopened_default.telemetry_export_configured);
    assert_eq!(
        reopened_default.telemetry_settings.mode,
        TelemetryMode::Default
    );
    let endpoint = format!("http://{address}/manual");
    let manual = run_command(
        &backend,
        &changed,
        ClientCommand::SetTelemetrySettings {
            mode: TelemetryMode::Manual,
            endpoint: endpoint.clone(),
            bearer: TelemetryBearerChange::Replace(OTLP_BEARER.into()),
        },
    );
    assert!(manual.telemetry_export_configured);
    assert_eq!(
        manual.telemetry_settings,
        TelemetrySettingsSummary {
            mode: TelemetryMode::Manual,
            endpoint: Some(endpoint),
            has_bearer_token: true,
        }
    );
    assert!(!format!("{manual:?}").contains(OTLP_BEARER));
    assert_rejected_configuration(&backend, &changed, address, &manual.telemetry_settings);
    run_command(
        &backend,
        &changed,
        ClientCommand::RefreshSubscription {
            subscription_uuid: uuid.clone(),
        },
    );
    backend
        .shutdown()
        .expect("flush telemetry and stop backend");
    assert!(!contains_bytes(
        &fs::read(data.join("catalog.sealed")).unwrap(),
        OTLP_BEARER
    ));

    let (mut backend, changed, reopened) = open_client(options);
    assert!(reopened.telemetry_export_configured);
    assert_eq!(reopened.telemetry_settings, manual.telemetry_settings);
    run_command(
        &backend,
        &changed,
        ClientCommand::RefreshSubscription {
            subscription_uuid: uuid,
        },
    );
    verify_credential_scope(&backend, &changed, address);
    backend.shutdown().expect("stop reopened backend");
}

fn assert_rejected_configuration(
    backend: &ClientHandle,
    changed: &mpsc::Receiver<()>,
    address: SocketAddr,
    previous: &TelemetrySettingsSummary,
) {
    let request = backend
        .send(ClientCommand::SetTelemetrySettings {
            mode: TelemetryMode::Manual,
            endpoint: format!("http://{address}/reject"),
            bearer: TelemetryBearerChange::Keep,
        })
        .expect("submit manual telemetry settings");
    let snapshot = wait_for_snapshot(backend, changed, |snapshot| {
        snapshot.completed_request == Some(request) && !snapshot.busy
    });
    assert_eq!(
        snapshot.last_error,
        Some(ClientError::TelemetryEndpointUnavailable)
    );
    assert_eq!(&snapshot.telemetry_settings, previous);
    assert!(snapshot.telemetry_export_configured);
}

fn verify_credential_scope(
    backend: &ClientHandle,
    changed: &mpsc::Receiver<()>,
    address: SocketAddr,
) {
    let same_endpoint = run_command(
        backend,
        changed,
        ClientCommand::SetTelemetrySettings {
            mode: TelemetryMode::Manual,
            endpoint: format!("HTTP://{address}/manual"),
            bearer: TelemetryBearerChange::Keep,
        },
    );
    assert!(same_endpoint.telemetry_settings.has_bearer_token);
    let moved_endpoint = run_command(
        backend,
        changed,
        ClientCommand::SetTelemetrySettings {
            mode: TelemetryMode::Manual,
            endpoint: format!("http://{address}/other"),
            bearer: TelemetryBearerChange::Keep,
        },
    );
    assert!(!moved_endpoint.telemetry_settings.has_bearer_token);
    let disabled = run_command(
        backend,
        changed,
        ClientCommand::SetTelemetrySettings {
            mode: TelemetryMode::Disabled,
            endpoint: String::new(),
            bearer: TelemetryBearerChange::Keep,
        },
    );
    assert!(!disabled.telemetry_export_configured);
    assert_eq!(
        disabled.telemetry_settings.endpoint,
        moved_endpoint.telemetry_settings.endpoint
    );
}

fn open_client(options: ClientCoreOptions) -> (ClientHandle, mpsc::Receiver<()>, ClientSnapshot) {
    let backend = ClientHandle::spawn(options).expect("spawn backend");
    let (notify, changed) = mpsc::sync_channel(1);
    backend.set_notifier(Arc::new(move || {
        let _ = notify.try_send(());
    }));
    let started = wait_for_snapshot(&backend, &changed, |snapshot| !snapshot.busy);
    assert_eq!(started.last_error, None);
    (backend, changed, started)
}

fn run_command(
    backend: &ClientHandle,
    changed: &mpsc::Receiver<()>,
    command: ClientCommand,
) -> ClientSnapshot {
    let request = backend.send(command).expect("submit backend command");
    let snapshot = wait_for_snapshot(backend, changed, |snapshot| {
        snapshot.completed_request == Some(request) && !snapshot.busy
    });
    assert_eq!(snapshot.last_error, None);
    snapshot
}

fn wait_for_snapshot(
    backend: &ClientHandle,
    changed: &mpsc::Receiver<()>,
    condition: impl Fn(&ClientSnapshot) -> bool,
) -> ClientSnapshot {
    let deadline = Instant::now() + SOCKET_TIMEOUT;
    loop {
        let snapshot = backend.snapshot();
        if condition(&snapshot) {
            return snapshot;
        }
        changed
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .expect("backend published the expected snapshot");
    }
}

#[derive(Default)]
struct Capture {
    exports: BTreeMap<String, Vec<u8>>,
    verifications: BTreeSet<String>,
    served_subscription: bool,
    device_id: Option<String>,
}

fn collect(listener: &TcpListener, child: &mut Child) -> Capture {
    let mut capture = Capture::default();
    let deadline = Instant::now() + TEST_TIMEOUT;
    loop {
        assert!(
            Instant::now() < deadline,
            "backend telemetry subprocess timed out"
        );
        match listener.accept() {
            Ok((stream, _)) => capture.serve(stream),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                if let Some(status) = child.try_wait().expect("check backend subprocess") {
                    assert!(status.success(), "backend subprocess failed: {status}");
                    return capture;
                }
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => panic!("accept local HTTP request: {error}"),
        }
    }
}

impl Capture {
    fn serve(&mut self, mut stream: TcpStream) {
        stream.set_nonblocking(false).unwrap();
        stream.set_read_timeout(Some(SOCKET_TIMEOUT)).unwrap();
        stream.set_write_timeout(Some(SOCKET_TIMEOUT)).unwrap();
        let request = read_request(&stream);
        if request.method == "GET" {
            assert_eq!(
                request.path,
                format!("/subscription?key={SUBSCRIPTION_TOKEN}")
            );
            self.device_id = request.headers.get("x-hwid").cloned();
            self.served_subscription = true;
            let body = format!(
                "socks5://synthetic-user:{PROFILE_TOKEN}@127.0.0.1:19002#{PROFILE_TITLE}\n"
            );
            respond(&mut stream, "text/plain", body.as_bytes());
        } else {
            assert_eq!(request.method, "POST");
            check_export_request(&request);
            assert_eq!(request.headers["content-type"], OTLP_CONTENT_TYPE);
            if request.body.is_empty() {
                self.serve_verification(&mut stream, request.path);
                return;
            }
            self.exports
                .entry(request.path)
                .or_default()
                .extend(request.body);
            respond(&mut stream, OTLP_CONTENT_TYPE, &[]);
        }
    }

    fn serve_verification(&mut self, stream: &mut TcpStream, path: String) {
        assert!(path.ends_with("/v1/logs"));
        if path == "/reject/v1/logs" {
            respond(stream, "text/html", b"<html>not an OTLP collector</html>");
        } else {
            respond(stream, OTLP_CONTENT_TYPE, &[]);
        }
        self.verifications.insert(path);
    }
}

fn check_export_request(request: &HttpRequest) {
    let (prefix, signal) = request.path.rsplit_once("/v1/").expect("OTLP signal path");
    assert!(matches!(signal, "traces" | "metrics" | "logs"));
    let authorization = request.headers.get("authorization");
    assert!(!request.headers.contains_key("x-sdk-fixture"));
    match prefix {
        "/manual" => {
            assert!(authorization.is_some_and(|value| value == &format!("Bearer {OTLP_BEARER}")))
        }
        "/operator" => {
            assert!(
                authorization.is_some_and(|value| value == &format!("Bearer {OPERATOR_BEARER}"))
            )
        }
        "/other" | "/reject" => assert!(authorization.is_none()),
        _ => panic!("unexpected OTLP endpoint prefix"),
    }
}

struct HttpRequest {
    method: String,
    path: String,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
}

fn read_request(stream: &TcpStream) -> HttpRequest {
    let mut stream = BufReader::new(stream);
    let line = read_line(&mut stream);
    let mut fields = line.split_ascii_whitespace();
    let method = fields.next().expect("HTTP method").into();
    let path = fields.next().expect("HTTP path").into();
    let headers = read_headers(&mut stream);
    let length: usize = headers
        .get("content-length")
        .map_or(0, |value| value.parse().unwrap());
    assert!(length <= 256 * 1024, "fixture request body too large");
    let mut body = vec![0; length];
    stream.read_exact(&mut body).expect("read HTTP body");
    HttpRequest {
        method,
        path,
        headers,
        body,
    }
}

fn read_headers(stream: &mut impl BufRead) -> BTreeMap<String, String> {
    let mut headers = BTreeMap::new();
    for _ in 0..64 {
        let line = read_line(stream);
        if line.is_empty() {
            return headers;
        }
        let (name, value) = line.split_once(':').expect("HTTP header pair");
        headers.insert(name.to_ascii_lowercase(), value.trim().to_owned());
    }
    panic!("fixture received too many HTTP headers");
}

fn read_line(stream: &mut impl BufRead) -> String {
    let mut line = String::new();
    stream
        .take(4096)
        .read_line(&mut line)
        .expect("read HTTP line");
    assert!(line.ends_with("\r\n"), "incomplete HTTP line");
    line.truncate(line.len() - 2);
    line
}

fn respond(stream: &mut TcpStream, media_type: &str, body: &[u8]) {
    write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: {media_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).unwrap();
    stream.write_all(body).expect("write HTTP response");
}

fn expect_bytes(payload: &[u8], value: &str) {
    assert!(contains_bytes(payload, value), "OTLP payload lacks {value}");
}

fn contains_bytes(payload: &[u8], value: &str) -> bool {
    payload
        .windows(value.len())
        .any(|window| window == value.as_bytes())
}

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "blazer-otlp-test-{}-{timestamp}",
            std::process::id()
        ));
        fs::create_dir(&dir).expect("create isolated backend test directory");
        Self(dir)
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct ClientProcess(Child);

impl Drop for ClientProcess {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
