#![cfg_attr(windows, windows_subsystem = "windows")]

mod capture;
mod demo;

use std::ffi::OsStr;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
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

const USAGE: &str = "usage: vpn-client-demo [--screenshot <file.png> [screen]]";

fn native_options() -> eframe::NativeOptions {
    let mut options = vpn_client::native_options();
    options.viewport = options.viewport.with_app_id("vpn-client-demo");
    options.persist_window = false;
    options
}
