use std::process::ExitCode;

use vpn_backend::{Backend, ConnectionState, EngineError};

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    match arguments.as_slice() {
        [command] if command == "status" => {
            print_status();
            ExitCode::SUCCESS
        }
        [] => {
            print_help();
            ExitCode::SUCCESS
        }
        [command] if command == "--help" || command == "-h" => {
            print_help();
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("Unsupported command. Use: vpn-headless status");
            ExitCode::from(2)
        }
    }
}

fn print_help() {
    println!("VPN-клиент — headless foundation");
    println!("Usage: vpn-headless status");
}

fn print_status() {
    let backend = Backend::new();
    let snapshot = backend.snapshot();
    let state = match snapshot.connection {
        Ok(ConnectionState::Disconnected) => "disconnected",
        Ok(ConnectionState::Connected) => "connected",
        Err(EngineError::EngineUnavailable) => "unavailable",
        Err(EngineError::UnsupportedFeature) => "unsupported",
    };
    println!("engine={state}");
    println!("connect_enabled={}", snapshot.connect_enabled);
    println!("telemetry_export=not_configured");
}
