//! Use cases and ports, independent of GUI and operating-system APIs.

use vpn_domain::ConnectionState;

/// The backend cannot currently report a tunnel state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TunnelUnavailable;

/// An output port implemented by a platform-specific tunnel adapter.
pub trait TunnelStatus {
    fn connection_state(&self) -> Result<ConnectionState, TunnelUnavailable>;
}

/// Read the tunnel state without changing network configuration.
pub struct GetConnectionStatus<P> {
    tunnel: P,
}

impl<P: TunnelStatus> GetConnectionStatus<P> {
    pub fn new(tunnel: P) -> Self {
        Self { tunnel }
    }

    pub fn execute(&self) -> Result<ConnectionState, TunnelUnavailable> {
        self.tunnel.connection_state()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct ReportedState(Result<ConnectionState, TunnelUnavailable>);

    impl TunnelStatus for ReportedState {
        fn connection_state(&self) -> Result<ConnectionState, TunnelUnavailable> {
            self.0
        }
    }

    #[test]
    fn preserves_the_backend_state_without_assuming_a_connection() {
        for state in [ConnectionState::Disconnected, ConnectionState::Connected] {
            let query = GetConnectionStatus::new(ReportedState(Ok(state)));
            assert_eq!(query.execute(), Ok(state));
        }
    }

    #[test]
    fn unavailable_backend_is_not_reported_as_disconnected() {
        let query = GetConnectionStatus::new(ReportedState(Err(TunnelUnavailable)));
        assert_eq!(query.execute(), Err(TunnelUnavailable));
    }
}
