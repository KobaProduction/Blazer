use std::io;

use client_ui::{BlazerView, UiHost};
use vpn_application::client::ClientEnginePort;
#[cfg(not(feature = "packet-tunnel"))]
use vpn_application::client::{Cancellation, EngineRuntime, ProxyProfile};
#[cfg(not(feature = "packet-tunnel"))]
use vpn_infrastructure::client_engine::validate_profile;
use vpn_presentation::localization::{self, Key, Locale};
use vpn_presentation::model::error_key;
#[cfg(not(feature = "packet-tunnel"))]
use vpn_presentation::session::{ClientError, ConnectionMode, ModeCapabilities};

pub fn run() -> eframe::Result {
    let title = localization::text(Locale::Russian, Key::AppName);
    crate::window::run(title, "com.kobaproduction.blazer", |context| {
        let controller = crate::session::open(context, |_| Ok(tunnel())).map_err(|error| {
            io::Error::other(localization::text(Locale::Russian, error_key(error)))
        })?;
        let host = UiHost {
            device_info: Some(format!("iOS ({})", std::env::consts::ARCH)),
            ..UiHost::default()
        };
        Ok(BlazerView::new(context, controller, host))
    })
}

#[cfg(feature = "packet-tunnel")]
fn tunnel() -> Box<dyn ClientEnginePort> {
    Box::new(crate::tunnel_control::AppleTunnel::default())
}

#[cfg(not(feature = "packet-tunnel"))]
fn tunnel() -> Box<dyn ClientEnginePort> {
    Box::new(UnavailableTunnel)
}

#[cfg(not(feature = "packet-tunnel"))]
struct UnavailableTunnel;

#[cfg(not(feature = "packet-tunnel"))]
impl ClientEnginePort for UnavailableTunnel {
    fn capabilities(&self) -> ModeCapabilities {
        ModeCapabilities::default()
    }

    fn validate_profile(&self, profile: &ProxyProfile) -> Result<(), ClientError> {
        validate_profile(profile)
    }

    fn start(
        &mut self,
        _mode: ConnectionMode,
        _profile: &ProxyProfile,
        _local_port: u16,
        _cancel: &Cancellation,
    ) -> Result<(), ClientError> {
        Err(ClientError::UnsupportedMode)
    }

    fn stop(&mut self) -> Result<(), ClientError> {
        Ok(())
    }

    fn poll(&mut self) -> Result<EngineRuntime, ClientError> {
        Ok(EngineRuntime::Stopped)
    }
}
