use std::net::TcpListener;
use std::process::Command;

#[test]
fn status_reports_unavailable_without_using_a_gui_or_otel_endpoint() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("local test listener");
    listener
        .set_nonblocking(true)
        .expect("nonblocking listener");
    let endpoint = format!(
        "http://{}",
        listener.local_addr().expect("listener address")
    );
    let output = Command::new(env!("CARGO_BIN_EXE_vpn-headless"))
        .arg("status")
        .env("OTEL_EXPORTER_OTLP_ENDPOINT", &endpoint)
        .env("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT", &endpoint)
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

#[test]
fn unsupported_command_is_not_silently_successful() {
    let output = Command::new(env!("CARGO_BIN_EXE_vpn-headless"))
        .arg("connect")
        .output()
        .expect("headless process starts");
    assert_eq!(output.status.code(), Some(2));
}
