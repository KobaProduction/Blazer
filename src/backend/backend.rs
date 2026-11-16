//! Shared outer composition root for native GUI, Android and headless delivery.

mod client;
mod telemetry;

pub use client::*;
pub use vpn_application::MAX_SUBSCRIPTION_BYTES;
