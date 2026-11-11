//! Outer adapters. No VPN driver or network mutation is implemented yet.

use vpn_application::{TunnelStatus, TunnelUnavailable};
use vpn_domain::ConnectionState;

/// Explicitly unavailable until a real platform adapter is implemented.
pub struct UnavailableTunnel;

impl TunnelStatus for UnavailableTunnel {
    fn connection_state(&self) -> Result<ConnectionState, TunnelUnavailable> {
        Err(TunnelUnavailable)
    }
}
