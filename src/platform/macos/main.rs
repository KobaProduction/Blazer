#[cfg(target_os = "macos")]
mod app;

#[cfg(all(target_os = "macos", feature = "packet-tunnel"))]
mod engine;

#[cfg(all(target_os = "macos", feature = "packet-tunnel"))]
#[path = "../apple/tunnel_control.rs"]
mod tunnel_control;

#[cfg(target_os = "macos")]
#[path = "../apple/catalog.rs"]
mod catalog;

#[cfg(target_os = "macos")]
#[path = "../apple/session.rs"]
mod session;

#[cfg(target_os = "macos")]
#[path = "../apple/window.rs"]
mod window;

#[cfg(target_os = "macos")]
fn main() -> eframe::Result {
    app::run()
}

#[cfg(not(target_os = "macos"))]
fn main() -> std::process::ExitCode {
    eprintln!("Build and run Blazer on macOS.");
    std::process::ExitCode::FAILURE
}
