//! Connection mode and engine lifecycle commands.

use crate::session::RoutingReconnect;

use super::{
    ActiveConnection, Cancellation, ClientApplication, ClientError, ConnectionMode, EngineRuntime,
};

impl ClientApplication {
    pub fn set_local_proxy_port(&mut self, port: u16) -> Result<(), ClientError> {
        if !self.capabilities().supports(ConnectionMode::LocalProxy) {
            return Err(ClientError::UnsupportedMode);
        }
        if self.catalog.settings.local_proxy_port == port {
            return Ok(());
        }
        let mut candidate = self.catalog.clone();
        candidate.settings.local_proxy_port = port;
        self.commit(candidate)
    }

    pub fn set_connection_mode(&mut self, mode: ConnectionMode) -> Result<(), ClientError> {
        if !self.capabilities().supports(mode) {
            return Err(ClientError::UnsupportedMode);
        }
        if self.active.is_some() && self.catalog.settings.connection_mode != mode {
            return Err(ClientError::DisconnectFirst);
        }
        if self.catalog.settings.connection_mode == mode {
            return Ok(());
        }
        let mut candidate = self.catalog.clone();
        candidate.settings.connection_mode = mode;
        self.commit(candidate)
    }

    pub fn connect(
        &mut self,
        mode: ConnectionMode,
        cancel: &Cancellation,
    ) -> Result<(), ClientError> {
        if !self.capabilities().supports(mode) {
            return Err(ClientError::UnsupportedMode);
        }
        if self.active.is_some() {
            return Err(ClientError::DisconnectFirst);
        }
        let selected = self
            .catalog
            .selected_profile_uuid
            .clone()
            .ok_or(ClientError::NoProfileSelected)?;
        self.start_profile(&selected, mode, cancel)
    }

    pub fn reconnect_routing(
        &mut self,
        pending: &RoutingReconnect,
        cancel: &Cancellation,
        on_start: impl FnOnce(&Self),
    ) -> Result<(), ClientError> {
        if self.routing_reconnect_pending().as_ref() != Some(pending) {
            return Err(ClientError::InvalidInput);
        }
        let active = self.active.as_ref().ok_or(ClientError::InvalidInput)?;
        let profile_uuid = active.profile_uuid.clone();
        let mode = active.mode;
        cancel.check()?;
        on_start(self);
        self.disconnect()?;
        self.start_profile(&profile_uuid, mode, cancel)
    }

    fn start_profile(
        &mut self,
        profile_uuid: &str,
        mode: ConnectionMode,
        cancel: &Cancellation,
    ) -> Result<(), ClientError> {
        let profile = self
            .catalog
            .profiles
            .iter()
            .find(|item| item.profile.profile_uuid == profile_uuid)
            .ok_or(ClientError::ProfileNotFound)?;
        let routing = self
            .catalog
            .routing
            .effective(profile.subscription_uuid.as_deref());
        cancel.check()?;
        self.services.engine.start_with_routing(
            mode,
            &profile.profile,
            self.catalog.settings.local_proxy_port,
            routing,
            cancel,
        )?;
        self.active = Some(ActiveConnection {
            connection_id: self.services.uuids.new_uuid(),
            profile_uuid: profile.profile.profile_uuid.clone(),
            mode,
            routing: routing.cloned(),
        });
        Ok(())
    }

    pub fn disconnect(&mut self) -> Result<(), ClientError> {
        self.services.engine.stop()?;
        self.active = None;
        Ok(())
    }

    pub fn poll_engine(&mut self) -> Result<EngineRuntime, ClientError> {
        let runtime = self.services.engine.poll()?;
        match runtime {
            EngineRuntime::Stopped => self.active = None,
            EngineRuntime::Running(connection) if self.active_mode() != Some(connection.mode()) => {
                return Err(ClientError::EngineReadiness);
            }
            EngineRuntime::Running(_) => {}
        }
        Ok(runtime)
    }
}
