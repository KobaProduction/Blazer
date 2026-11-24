//! Contracts supplied by infrastructure and platform composition roots.

use std::net::SocketAddr;
use std::time::Duration;

use super::{Cancellation, Catalog, ClientError, ConnectionMode, ProxyProfile, UnavailableProfile};
use crate::provider::{FetchError, FetchOutcome, FetchRequest};
use crate::session::{ModeCapabilities, RoutingProfile};
use vpn_domain::client::ProbeSettings;
use vpn_domain::provider::{ProviderEndpoint, ProviderMetadata};

pub trait StateStorePort: Send {
    fn load(&self) -> Result<Option<Catalog>, ClientError>;
    fn save(&self, catalog: &Catalog) -> Result<(), ClientError>;
}

pub trait SubscriptionSourcePort: Send {
    fn provider_title(&self, url: &str, metadata: &ProviderMetadata) -> Option<String>;
    fn endpoint(&self, url: &str) -> Result<ProviderEndpoint, ClientError>;
    fn replace_domain(
        &self,
        primary: &ProviderEndpoint,
        domain: &str,
    ) -> Result<ProviderEndpoint, ClientError>;
    fn fetch(
        &self,
        request: FetchRequest<'_>,
        cancel: &Cancellation,
    ) -> Result<FetchOutcome, FetchError>;
}

pub trait ClientNormalizerPort: Send {
    /// Translates supported input formats into profiles for the application.
    fn normalize(&self, input: &[u8]) -> Result<Vec<ProxyProfile>, ClientError>;

    fn normalize_subscription(&self, input: &[u8]) -> Result<NormalizedSubscription, ClientError>;
}

#[derive(Debug, Default)]
pub struct NormalizedSubscription {
    pub profiles: Vec<ProxyProfile>,
    pub unavailable_profiles: Vec<UnavailableProfile>,
}

pub trait UuidPort: Send {
    fn new_uuid(&self) -> String;
}

/// An owned, cancellable measurement. It must release its resources before returning.
pub type OwnedProbeTask = Box<dyn FnOnce(&Cancellation) -> Result<Duration, ClientError> + Send>;
pub const MAX_CONCURRENT_PROFILE_PROBES: usize = 5;

pub trait ProfileProbePort: Send {
    fn prepare_probe(
        &self,
        _host: String,
        _port: u16,
        _settings: ProbeSettings,
    ) -> Result<OwnedProbeTask, ClientError> {
        Err(ClientError::UnsupportedFeature)
    }
    fn supports_icmp(&self) -> bool {
        false
    }
    /// Each result corresponds to the endpoint at the same index.
    fn connect_times(
        &self,
        endpoints: &[(&str, u16)],
        settings: &ProbeSettings,
        cancel: &Cancellation,
    ) -> Result<Vec<Result<Duration, ClientError>>, ClientError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineRuntime {
    Stopped,
    Running(RunningConnection),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunningConnection {
    LocalProxy(SocketAddr),
    SystemProxy(SocketAddr),
    Tun,
}

impl RunningConnection {
    pub const fn mode(self) -> ConnectionMode {
        match self {
            Self::LocalProxy(_) => ConnectionMode::LocalProxy,
            Self::SystemProxy(_) => ConnectionMode::SystemProxy,
            Self::Tun => ConnectionMode::Tun,
        }
    }

    pub const fn proxy_address(self) -> Option<SocketAddr> {
        match self {
            Self::LocalProxy(address) | Self::SystemProxy(address) => Some(address),
            Self::Tun => None,
        }
    }
}

pub trait ClientEnginePort: Send {
    fn prepare_profile_probe(
        &self,
        _profile: &ProxyProfile,
        _active: Option<&ProxyProfile>,
        _reuse_active_proxy: bool,
        _settings: &ProbeSettings,
    ) -> Result<OwnedProbeTask, ClientError> {
        Err(ClientError::UnsupportedFeature)
    }
    fn capabilities(&self) -> ModeCapabilities;
    /// Checks that this engine can represent the profile without starting a connection.
    fn validate_profile(&self, profile: &ProxyProfile) -> Result<(), ClientError>;
    fn supports_tunnel_probe(&self) -> bool {
        false
    }
    /// Can measure the current TUN connection without starting another tunnel.
    fn supports_active_tunnel_probe(&self) -> bool {
        false
    }
    fn probe_tunnel(
        &self,
        _profile: &ProxyProfile,
        _active: Option<&ProxyProfile>,
        _reuse_active_proxy: bool,
        _settings: &ProbeSettings,
        _cancel: &Cancellation,
    ) -> Result<Duration, ClientError> {
        Err(ClientError::UnsupportedFeature)
    }
    fn start(
        &mut self,
        mode: ConnectionMode,
        profile: &ProxyProfile,
        local_port: u16,
        cancel: &Cancellation,
    ) -> Result<(), ClientError>;
    fn supports_routing(&self, _mode: ConnectionMode) -> bool {
        false
    }
    fn start_with_routing(
        &mut self,
        mode: ConnectionMode,
        profile: &ProxyProfile,
        local_port: u16,
        routing: Option<&RoutingProfile>,
        cancel: &Cancellation,
    ) -> Result<(), ClientError> {
        if routing.is_some() {
            return Err(ClientError::UnsupportedMode);
        }
        self.start(mode, profile, local_port, cancel)
    }
    fn stop(&mut self) -> Result<(), ClientError>;
    fn poll(&mut self) -> Result<EngineRuntime, ClientError>;
}

pub struct ClientServices {
    pub store: Box<dyn StateStorePort>,
    pub source: Box<dyn SubscriptionSourcePort>,
    pub normalizer: Box<dyn ClientNormalizerPort>,
    pub uuids: Box<dyn UuidPort>,
    pub profile_probe: Box<dyn ProfileProbePort>,
    pub engine: Box<dyn ClientEnginePort>,
}
