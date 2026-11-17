#[cfg(target_os = "ios")]
#[path = "../apple/preview.rs"]
mod preview;

#[cfg(target_os = "ios")]
fn main() -> eframe::Result {
    preview::run()
}

#[cfg(not(target_os = "ios"))]
fn main() -> std::process::ExitCode {
    eprintln!("Build blazer-ios-preview using the iOS SDK from Xcode.");
    std::process::ExitCode::FAILURE
}
