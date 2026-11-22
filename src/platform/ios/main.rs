#[cfg(target_os = "ios")]
mod app;

#[cfg(target_os = "ios")]
#[path = "../apple/catalog.rs"]
mod catalog;

#[cfg(target_os = "ios")]
#[path = "../apple/session.rs"]
mod session;

#[cfg(all(target_os = "ios", feature = "packet-tunnel"))]
#[path = "../apple/tunnel_control.rs"]
mod tunnel_control;

#[cfg(target_os = "ios")]
#[path = "../apple/window.rs"]
mod window;

#[cfg(target_os = "ios")]
fn main() -> eframe::Result {
    app::run()
}

#[cfg(not(target_os = "ios"))]
fn main() -> std::process::ExitCode {
    eprintln!("Build Blazer using the iOS SDK from Xcode.");
    std::process::ExitCode::FAILURE
}
