#[cfg(target_os = "macos")]
#[path = "../apple/preview.rs"]
mod preview;

#[cfg(target_os = "macos")]
fn main() -> eframe::Result {
    preview::run()
}

#[cfg(not(target_os = "macos"))]
fn main() -> std::process::ExitCode {
    eprintln!("Build and run blazer-macos-preview on macOS.");
    std::process::ExitCode::FAILURE
}
