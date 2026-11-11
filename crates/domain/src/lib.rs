//! Platform-independent connection and configuration vocabulary.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    Disconnected,
    Connected,
}

/// Model vocabulary; a variant does not imply engine support.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtocolKind {
    WireGuard,
    AmneziaWireGuard,
    Vless,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionProfile {
    pub name: String,
    pub protocol: ProtocolKind,
}

/// A typed normalization result, never an opaque source document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedConfiguration {
    pub profiles: Vec<ConnectionProfile>,
}
