use std::io;

use client_ui::{BlazerView, UiHost};
use vpn_application::client::ClientEnginePort;
use vpn_infrastructure::client_engine::{BlazeCoreProcess, TrustedEngineSpec};
use vpn_presentation::localization::{self, Key, Locale};
use vpn_presentation::model::error_key;
use vpn_presentation::session::ClientError;

pub fn run() -> eframe::Result {
    let title = localization::text(Locale::Russian, Key::AppName);
    crate::window::run(title, "com.kobaproduction.blazer", |context| {
        let controller = crate::session::open(context, |directory| {
            let engine = BlazeCoreProcess::new(engine_spec()?, directory.to_owned(), 0);
            Ok(platform_engine(engine))
        })
        .map_err(|error| io::Error::other(localization::text(Locale::Russian, error_key(error))))?;
        let host = UiHost {
            device_info: Some(format!("macOS ({})", std::env::consts::ARCH)),
            ..UiHost::default()
        };
        Ok(BlazerView::new(context, controller, host))
    })
}

fn platform_engine(proxy: BlazeCoreProcess) -> Box<dyn ClientEnginePort> {
    #[cfg(feature = "packet-tunnel")]
    return Box::new(crate::engine::MacEngine::new(proxy));

    #[cfg(not(feature = "packet-tunnel"))]
    Box::new(proxy)
}

fn engine_spec() -> Result<TrustedEngineSpec, ClientError> {
    let app = std::env::current_exe().map_err(|_| ClientError::EngineNotFound)?;
    Ok(TrustedEngineSpec {
        source_path: app.with_file_name("blazecore"),
        expected_sha256: option_env!("BLAZER_MACOS_CORE_SHA256").unwrap_or("").into(),
        expected_version: option_env!("BLAZER_MACOS_CORE_VERSION")
            .unwrap_or("")
            .into(),
    })
}
