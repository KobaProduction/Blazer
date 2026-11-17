#![cfg(windows)]

use std::fs;
use std::io::Write;
#[cfg(windows)]
use std::io::{BufRead, BufReader};
use std::net::TcpListener;
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

#[cfg(windows)]
#[path = "fixtures/routing.rs"]
mod routing;

struct TempData(PathBuf);

fn headless_command() -> Command {
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let mut command = Command::new(env!("CARGO_BIN_EXE_blazer"));
    command.arg("--cli").creation_flags(CREATE_NO_WINDOW);
    command
}

impl TempData {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let directory = std::env::temp_dir().join(format!(
            "blazer-headless-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&directory).expect("fresh test catalog directory");
        Self(directory)
    }

    fn command(&self) -> Command {
        let mut command = headless_command();
        command
            .env("BLAZER_DATA_DIR", &self.0)
            .env(
                "BLAZER_BLAZECORE_PATH",
                self.0.join("missing-blazecore.exe"),
            )
            .env("OTEL_SDK_DISABLED", "true");
        command
    }

    #[cfg(windows)]
    fn run(&self, args: &[&str], input: Option<&[u8]>) -> String {
        let mut child = self
            .command()
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(input.unwrap_or_default()).unwrap();
        }
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }
}

impl Drop for TempData {
    fn drop(&mut self) {
        for file in ["catalog.sealed", "catalog.lock", "subscription-device-id"] {
            let _ = fs::remove_file(self.0.join(file));
        }
        let _ = fs::remove_dir(&self.0);
    }
}

#[test]
fn status_reports_unavailable_without_contacting_a_disabled_otel_endpoint() {
    let data = TempData::new();
    let listener = TcpListener::bind("127.0.0.1:0").expect("local test listener");
    listener
        .set_nonblocking(true)
        .expect("nonblocking listener");
    let endpoint = format!(
        "http://{}",
        listener.local_addr().expect("listener address")
    );
    let output = data
        .command()
        .arg("status")
        .env("OTEL_ENDPOINT", &endpoint)
        .env("OTEL_BEARER_TOKEN", "synthetic-disabled-bearer")
        .env("OTEL_EXPORTER_OTLP_ENDPOINT", &endpoint)
        .env("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT", &endpoint)
        .env("OTEL_EXPORTER_OTLP_METRICS_ENDPOINT", &endpoint)
        .env("OTEL_EXPORTER_OTLP_LOGS_ENDPOINT", &endpoint)
        .env("OTEL_TRACES_EXPORTER", "otlp")
        .env("OTEL_TRACES_SAMPLER", "always_on")
        .output()
        .expect("headless process starts");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("UTF-8 status output");
    assert!(stdout.contains("engine=unavailable"));
    assert!(stdout.contains("connect_enabled=false"));
    assert!(stdout.contains("telemetry_export=not_configured"));
    assert_eq!(
        listener
            .accept()
            .expect_err("no exporter should connect")
            .kind(),
        std::io::ErrorKind::WouldBlock
    );
}

#[cfg(windows)]
#[test]
fn stdin_import_persists_profiles_without_echoing_credentials() {
    let data = TempData::new();
    let fixture = b"socks5://fixture-user:fixture-password@127.0.0.1:1234#Fixture\n";
    let mut child = data
        .command()
        .arg("import")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("headless import starts");
    child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(fixture)
        .expect("write synthetic profile");
    let output = child.wait_with_output().expect("headless import exits");
    assert!(output.status.success(), "{:?}", output.stderr);
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "import=ok");
    let persisted = fs::read(data.0.join("catalog.sealed")).expect("sealed catalog");
    assert!(persisted.starts_with(b"VPNC01\0"));
    let password = b"fixture-password";
    assert!(
        !persisted
            .windows(password.len())
            .any(|value| value == password)
    );
}

#[test]
fn unsupported_command_is_not_silently_successful() {
    let output = headless_command()
        .arg("connect")
        .output()
        .expect("headless process starts");
    assert_eq!(output.status.code(), Some(2));
}

#[cfg(windows)]
#[test]
fn headless_catalog_selects_probes_and_removes_profiles_across_restarts() {
    let data = TempData::new();
    let target = TcpListener::bind("127.0.0.1:0").unwrap();
    let fixture = format!(
        "socks5://fixture-user:fixture-password@{}#First\nsocks5://127.0.0.1:1#Second",
        target.local_addr().unwrap(),
    );
    assert_eq!(
        data.run(&["import"], Some(fixture.as_bytes())).trim(),
        "import=ok"
    );
    let listed = data.run(&["profiles"], None);
    assert!(!listed.contains("fixture-password"));
    let names = |name: &str| {
        listed
            .lines()
            .find(|line| line.ends_with(name))
            .and_then(|line| line.split_whitespace().next())
            .unwrap()
            .strip_prefix("profile=")
            .unwrap()
    };
    let first = names("\"First\"");
    let second = names("\"Second\"");
    assert!(
        data.run(&["profile", "test", first], None)
            .starts_with("probe=reachable milliseconds=")
    );
    let batch = data.run(&["profiles", "test"], None);
    assert!(batch.lines().any(|line| line == "probed=2"));
    assert!(batch.contains(&format!("profile={first} probe=reachable milliseconds=")));
    assert!(batch.contains(&format!("profile={second} probe=")));
    data.run(&["profile", "select", second], None);
    assert!(
        data.run(&["profiles"], None)
            .contains(&format!("selected_profile_uuid={second}"))
    );
    data.run(&["profile", "remove", first], None);
    assert!(!data.run(&["profiles"], None).contains(first));
    data.run(&["language", "en"], None);
    data.run(&["mode", "proxy"], None);
    data.run(&["local-proxy-port", "32108"], None);
    let settings = data.run(&["settings"], None);
    assert!(settings.contains("language=en") && settings.contains("mode=proxy"));
    assert!(settings.contains("local_proxy_port=32108"));
}

#[cfg(windows)]
#[test]
fn headless_subscription_renames_without_fetching_or_exposing_the_url() {
    let data = TempData::new();
    let source = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!(
        "http://{}/subscription?key=fixture-secret",
        source.local_addr().unwrap()
    );
    add_subscription_fixture(&data, &source, &url);
    let listed = data.run(&["subscriptions"], None);
    assert!(!listed.contains("fixture-secret"));
    let uuid = listed
        .split_whitespace()
        .next()
        .unwrap()
        .strip_prefix("subscription=")
        .unwrap();
    source.set_nonblocking(true).unwrap();
    data.run(&["subscription", "rename", uuid, "After"], None);
    let renamed = data.run(&["subscriptions"], None);
    assert!(renamed.contains(&format!("subscription={uuid} profiles=1 title=\"After\"")));
    let batch = data.run(&["subscription", "test", uuid], None);
    assert!(batch.lines().any(|line| line == "probed=1"));
    assert_eq!(
        source.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    data.run(&["subscription", "remove", uuid], None);
    assert!(data.run(&["subscriptions"], None).is_empty());
    assert!(data.run(&["profiles"], None).is_empty());
}

#[cfg(windows)]
fn add_subscription_fixture(data: &TempData, source: &TcpListener, url: &str) {
    source.set_nonblocking(true).unwrap();
    std::thread::scope(|scope| {
        scope.spawn(|| {
            let limit = std::time::Instant::now() + std::time::Duration::from_secs(5);
            let socket = loop {
                match source.accept() {
                    Ok((socket, _)) => break socket,
                    Err(error)
                        if error.kind() == std::io::ErrorKind::WouldBlock
                            && std::time::Instant::now() < limit =>
                    {
                        std::thread::sleep(std::time::Duration::from_millis(10));
                    }
                    Err(error) => panic!("subscription fixture could not accept: {error}"),
                }
            };
            socket.set_nonblocking(false).unwrap();
            socket
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let mut request = BufReader::new(socket);
            let mut line = String::new();
            request.read_line(&mut line).unwrap();
            assert!(line.starts_with("GET /subscription?key=fixture-secret "));
            loop {
                line.clear();
                assert_ne!(request.read_line(&mut line).unwrap(), 0);
                if line == "\r\n" {
                    break;
                }
            }
            let body = "socks5://127.0.0.1:1080#Fixture\n";
            write!(
                request.get_mut(),
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
        });
        data.run(&["subscription", "add", "Before"], Some(url.as_bytes()));
    });
}
