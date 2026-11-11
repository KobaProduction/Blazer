//! Platform-independent vocabulary of a VPN connection.

/// A connection state reported by a tunnel backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    Disconnected,
    Connected,
}
