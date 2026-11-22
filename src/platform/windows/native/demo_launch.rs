#[path = "capture.rs"]
mod capture;
#[path = "demo.rs"]
mod demo;

use std::ffi::{OsStr, OsString};

pub fn run(mut args: impl Iterator<Item = OsString>) -> Result<(), Box<dyn std::error::Error>> {
    let command = args.next();
    match command.as_deref() {
        None => Ok(demo::run_demo()?),
        Some(command) if command == OsStr::new("--screenshot") => {
            let path = args.next().ok_or(USAGE)?;
            let screen = args.next().unwrap_or_else(|| "home".into());
            if args.next().is_some() {
                return Err(USAGE.into());
            }
            capture::capture(path.into(), &screen)
        }
        Some(_) => Err(USAGE.into()),
    }
}

const USAGE: &str = "usage: blazer --demo [--screenshot <file.png> [screen]]";

fn native_options() -> eframe::NativeOptions {
    let mut options = blazer::native_options();
    options.viewport = options.viewport.with_app_id("blazer-demo");
    options.persist_window = false;
    options
}
