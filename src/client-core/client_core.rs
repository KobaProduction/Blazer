//! Client runtime shared by native GUI, Android and headless entry points.

mod client;
mod telemetry;

pub use client::*;
pub use vpn_application::MAX_SUBSCRIPTION_BYTES;

#[cfg(windows)]
pub use vpn_infrastructure::client_engine::{recover_system_proxy, run_system_proxy_guard};
