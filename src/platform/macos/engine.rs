use std::time::Duration;

use vpn_application::client::{Cancellation, ClientEnginePort, EngineRuntime, ProxyProfile};
use vpn_infrastructure::client_engine::BlazeCoreProcess;
use vpn_presentation::session::{ClientError, ConnectionMode, ModeCapabilities, RoutingProfile};

use crate::tunnel_control::AppleTunnel;

pub struct MacEngine {
    proxy: BlazeCoreProcess,
    tunnel: AppleTunnel,
    active: Option<ConnectionMode>,
}

impl MacEngine {
    pub fn new(proxy: BlazeCoreProcess) -> Self {
        Self {
            proxy,
            tunnel: AppleTunnel::default(),
            active: None,
        }
    }

    fn adapter(&mut self, mode: ConnectionMode) -> &mut dyn ClientEnginePort {
        match mode {
            ConnectionMode::Tun => &mut self.tunnel,
            _ => &mut self.proxy,
        }
    }
}

impl ClientEnginePort for MacEngine {
    fn prepare_profile_probe(
        &self,
        profile: &ProxyProfile,
        active: Option<&ProxyProfile>,
        reuse: bool,
        settings: &vpn_presentation::session::ProbeSettings,
    ) -> Result<vpn_application::client::OwnedProbeTask, ClientError> {
        self.proxy
            .prepare_profile_probe(profile, active, reuse, settings)
    }
    fn capabilities(&self) -> ModeCapabilities {
        ModeCapabilities {
            tun: true,
            ..self.proxy.capabilities()
        }
    }

    fn validate_profile(&self, profile: &ProxyProfile) -> Result<(), ClientError> {
        self.proxy.validate_profile(profile)
    }

    fn supports_tunnel_probe(&self) -> bool {
        self.proxy.supports_tunnel_probe()
    }

    fn probe_tunnel(
        &self,
        profile: &ProxyProfile,
        active: Option<&ProxyProfile>,
        reuse_active_proxy: bool,
        settings: &vpn_presentation::session::ProbeSettings,
        cancel: &Cancellation,
    ) -> Result<Duration, ClientError> {
        self.proxy
            .probe_tunnel(profile, active, reuse_active_proxy, settings, cancel)
    }

    fn supports_routing(&self, mode: ConnectionMode) -> bool {
        self.capabilities().supports(mode)
    }

    fn start(
        &mut self,
        mode: ConnectionMode,
        profile: &ProxyProfile,
        port: u16,
        cancel: &Cancellation,
    ) -> Result<(), ClientError> {
        self.start_with_routing(mode, profile, port, None, cancel)
    }

    fn start_with_routing(
        &mut self,
        mode: ConnectionMode,
        profile: &ProxyProfile,
        port: u16,
        routing: Option<&RoutingProfile>,
        cancel: &Cancellation,
    ) -> Result<(), ClientError> {
        if self.active.is_some() {
            return Err(ClientError::DisconnectFirst);
        }
        self.adapter(mode)
            .start_with_routing(mode, profile, port, routing, cancel)?;
        self.active = Some(mode);
        Ok(())
    }

    fn stop(&mut self) -> Result<(), ClientError> {
        self.tunnel.stop()?;
        self.proxy.stop()?;
        self.active = None;
        Ok(())
    }

    fn poll(&mut self) -> Result<EngineRuntime, ClientError> {
        let Some(mode) = self.active else {
            return Ok(EngineRuntime::Stopped);
        };
        let state = self.adapter(mode).poll()?;
        if state == EngineRuntime::Stopped {
            self.active = None;
        }
        Ok(state)
    }
}
