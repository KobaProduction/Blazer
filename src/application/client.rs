//! Shared client use cases and their transactional catalog.

use std::collections::HashMap;

mod connection;
mod ports;
mod probe;
mod profiles;
mod routing;
mod subscriptions;

use vpn_domain::client::Secret;
pub use vpn_domain::client::{
    Catalog, CatalogProfile, ClientError, ClientSettings, ConnectionMode, Language, MAX_PROFILES,
    MAX_SUBSCRIPTIONS, ProfileUnavailableReason, ProxyProfile, Subscription, TelemetryMode,
    TelemetrySettings, UnavailableProfile,
};

pub use crate::cancellation::{Cancellation, CancellationSignal};
use crate::session::{ModeCapabilities, ProfileLatency, RoutingProfile, TelemetryBearerChange};
pub use ports::*;

pub struct ClientApplication {
    catalog: Catalog,
    services: ClientServices,
    active: Option<ActiveConnection>,
    profile_latencies: HashMap<String, ProfileLatency>,
    last_saved_routing_uuid: Option<String>,
}

struct ActiveConnection {
    connection_id: String,
    profile_uuid: String,
    mode: ConnectionMode,
    routing: Option<RoutingProfile>,
}

impl ClientApplication {
    pub fn load(services: ClientServices) -> Result<Self, ClientError> {
        let mut catalog = services.store.load()?.unwrap_or_default();
        catalog.validate()?;
        let capabilities = services.engine.capabilities();
        if !capabilities.supports(catalog.settings.connection_mode)
            && let Some(mode) = [
                ConnectionMode::LocalProxy,
                ConnectionMode::SystemProxy,
                ConnectionMode::Tun,
            ]
            .into_iter()
            .find(|mode| capabilities.supports(*mode))
        {
            catalog.settings.connection_mode = mode;
        }
        Ok(Self {
            catalog,
            services,
            active: None,
            profile_latencies: HashMap::new(),
            last_saved_routing_uuid: None,
        })
    }

    pub fn catalog(&self) -> &Catalog {
        &self.catalog
    }

    pub fn active_profile_uuid(&self) -> Option<&str> {
        self.active
            .as_ref()
            .map(|active| active.profile_uuid.as_str())
    }

    pub fn active_subscription_uuid(&self) -> Option<&str> {
        let active = self.active_profile_uuid()?;
        self.catalog
            .profiles
            .iter()
            .find(|item| item.profile.profile_uuid == active)?
            .subscription_uuid
            .as_deref()
    }

    pub fn active_mode(&self) -> Option<ConnectionMode> {
        self.active.as_ref().map(|active| active.mode)
    }

    pub fn capabilities(&self) -> ModeCapabilities {
        self.services.engine.capabilities()
    }

    fn commit(&mut self, candidate: Catalog) -> Result<(), ClientError> {
        candidate.validate()?;
        self.services.store.save(&candidate)?;
        self.profile_latencies.retain(|uuid, _| {
            candidate
                .profiles
                .iter()
                .any(|item| item.profile.profile_uuid == *uuid)
                || candidate
                    .subscriptions
                    .iter()
                    .flat_map(|subscription| &subscription.unavailable_profiles)
                    .any(|row| row.probe_id.as_ref() == Some(uuid))
        });
        self.catalog = candidate;
        Ok(())
    }

    pub fn set_language(&mut self, language: Language) -> Result<(), ClientError> {
        let mut candidate = self.catalog.clone();
        candidate.settings.language = language;
        self.commit(candidate)
    }

    pub fn set_telemetry_settings(
        &mut self,
        mode: TelemetryMode,
        endpoint: String,
        bearer: TelemetryBearerChange,
        cancel: &Cancellation,
        verify_manual: impl FnOnce(&TelemetrySettings, &Cancellation) -> Result<(), ClientError>,
    ) -> Result<(), ClientError> {
        cancel.check()?;
        let current = &self.catalog.settings.telemetry;
        let mut settings = current.clone();
        settings.mode = mode;
        if mode == TelemetryMode::Manual {
            let endpoint = endpoint.trim_matches(' ');
            TelemetrySettings::validate_endpoint(endpoint)?;
            let canonical = self.services.source.endpoint(endpoint)?;
            let canonical = canonical.url.expose();
            settings.bearer_token = match bearer {
                TelemetryBearerChange::Keep if settings.endpoint.as_deref() == Some(canonical) => {
                    settings.bearer_token
                }
                TelemetryBearerChange::Keep | TelemetryBearerChange::Clear => None,
                TelemetryBearerChange::Replace(token) => Some(Secret::new(token)),
            };
            settings.endpoint = Some(canonical.to_owned());
            settings.validate()?;
            verify_manual(&settings, cancel)?;
        }
        cancel.check()?;
        if &settings == current {
            return Ok(());
        }
        let mut candidate = self.catalog.clone();
        candidate.settings.telemetry = settings;
        self.commit(candidate)
    }
}
