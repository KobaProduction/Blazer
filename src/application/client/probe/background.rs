use super::*;
use crate::client::OwnedProbeTask;
use vpn_domain::client::{NativeProxyKind, ProxyProtocol};

pub struct PreparedProfileProbe {
    pub profile_uuid: String,
    pub method: ProbeMethod,
    pub exclusive: bool,
    pub revision: ProbeRevision,
    pub task: Result<OwnedProbeTask, ClientError>,
}

/// Opaque catalog/connection identity; never exposed to UI or telemetry.
#[derive(PartialEq, Eq)]
pub struct ProbeRevision {
    uuid: String,
    profile: Option<ProxyProfile>,
    endpoint: Option<(String, u16)>,
    settings: ProbeSettings,
    connection: Option<String>,
}

impl ClientApplication {
    pub fn prepare_profile_probes(
        &self,
        profile_uuid: Option<&str>,
        subscription_uuid: Option<&str>,
    ) -> Result<Vec<PreparedProfileProbe>, ClientError> {
        self.catalog.settings.probe.validate()?;
        if !self.can_probe_profile() {
            return Err(ClientError::UnsupportedMode);
        }
        if let Some(uuid) = subscription_uuid
            && !self
                .catalog
                .subscriptions
                .iter()
                .any(|item| item.subscription_uuid == uuid)
        {
            return Err(ClientError::SubscriptionNotFound);
        }
        let ids = if let Some(uuid) = profile_uuid {
            vec![uuid.to_owned()]
        } else {
            self.catalog
                .profiles
                .iter()
                .filter(|item| {
                    subscription_uuid
                        .is_none_or(|uuid| item.subscription_uuid.as_deref() == Some(uuid))
                })
                .map(|item| item.profile.profile_uuid.clone())
                .chain(
                    self.catalog
                        .subscriptions
                        .iter()
                        .filter(|item| {
                            subscription_uuid.is_none_or(|uuid| item.subscription_uuid == uuid)
                        })
                        .flat_map(|item| &item.unavailable_profiles)
                        .filter_map(|item| item.probe_id.clone()),
                )
                .collect()
        };
        if ids.is_empty() {
            return Err(ClientError::UnsupportedFeature);
        }
        let mut jobs = ids
            .into_iter()
            .map(|uuid| self.prepare_one_probe(uuid))
            .collect::<Vec<_>>();
        if profile_uuid.is_some()
            && let Err(error) = jobs[0].task
        {
            return Err(error);
        }
        if jobs.iter().all(|job| job.task.is_err()) && !self.use_active_tunnel_probe() {
            return Err(jobs
                .remove(0)
                .task
                .err()
                .expect("no eligible probe targets"));
        }
        Ok(jobs)
    }

    fn prepare_one_probe(&self, uuid: String) -> PreparedProfileProbe {
        let profile = self.saved_probe_profile(&uuid);
        let engine = self.use_active_tunnel_probe()
            || profile.is_some_and(|profile| self.needs_engine_probe(profile));
        let method = if engine {
            ProbeMethod::Proxy
        } else if self.catalog.settings.probe.method == ProbeMethod::Icmp {
            ProbeMethod::Icmp
        } else {
            ProbeMethod::Tcp
        };
        let endpoint = self
            .probe_target(&uuid)
            .ok()
            .map(|(_, (host, port))| (host.to_owned(), port));
        let revision = ProbeRevision {
            uuid: uuid.clone(),
            profile: profile.cloned(),
            endpoint,
            settings: self.catalog.settings.probe.clone(),
            connection: self.active.as_ref().map(|item| item.connection_id.clone()),
        };
        let task = self.prepare_probe_task(&uuid, profile, engine);
        PreparedProfileProbe {
            profile_uuid: uuid,
            method,
            exclusive: profile.is_some_and(wireguard_profile),
            revision,
            task,
        }
    }

    fn prepare_probe_task(
        &self,
        uuid: &str,
        profile: Option<&ProxyProfile>,
        engine: bool,
    ) -> Result<OwnedProbeTask, ClientError> {
        if self.use_active_tunnel_probe() && self.active_profile_uuid() != Some(uuid) {
            return Err(ClientError::DisconnectFirst);
        }
        if engine {
            let profile = profile.ok_or(ClientError::UnsupportedFeature)?;
            let supported = if self.active_mode() == Some(ConnectionMode::Tun) {
                self.services.engine.supports_active_tunnel_probe()
            } else {
                self.services.engine.supports_tunnel_probe()
            };
            if !supported {
                return Err(ClientError::UnsupportedFeature);
            }
            let active = self
                .active_profile_uuid()
                .and_then(|uuid| self.saved_probe_profile(uuid));
            let reuse = self.active_mode() != Some(ConnectionMode::Tun)
                && self
                    .active
                    .as_ref()
                    .is_some_and(|item| item.routing.is_none());
            return self.services.engine.prepare_profile_probe(
                profile,
                active,
                reuse,
                &self.catalog.settings.probe,
            );
        }
        let (_, (host, port)) = self.probe_target(uuid)?;
        self.services.profile_probe.prepare_probe(
            host.to_owned(),
            port,
            self.catalog.settings.probe.clone(),
        )
    }

    pub fn profile_probe_revision_is_current(&self, revision: &ProbeRevision) -> bool {
        if revision.settings != self.catalog.settings.probe
            || revision.connection.as_deref()
                != self.active.as_ref().map(|item| item.connection_id.as_str())
            || revision.profile.as_ref() != self.saved_probe_profile(&revision.uuid)
        {
            return false;
        }
        if revision.profile.is_none() {
            let endpoint = self
                .probe_target(&revision.uuid)
                .ok()
                .map(|(_, (host, port))| (host.to_owned(), port));
            if endpoint.is_none() || endpoint != revision.endpoint {
                return false;
            }
        }
        true
    }

    pub fn accept_profile_probe(
        &mut self,
        revision: &ProbeRevision,
        result: Result<Duration, ClientError>,
        method: ProbeMethod,
    ) -> Option<ProfileLatency> {
        if !self.profile_probe_revision_is_current(revision) {
            return None;
        }
        let value = match result {
            Err(ClientError::Network) if method == ProbeMethod::Proxy => {
                ProfileLatency::CheckUnreachable
            }
            result => latency(result).unwrap_or_else(ProfileLatency::Failed),
        };
        self.profile_latencies.insert(revision.uuid.clone(), value);
        Some(value)
    }
}

fn wireguard_profile(profile: &ProxyProfile) -> bool {
    match &profile.protocol {
        ProxyProtocol::Native(native) => matches!(native.kind, NativeProxyKind::WireGuard | NativeProxyKind::AmneziaWg),
        ProxyProtocol::Graph(graph) => graph.outbounds.iter().any(|outbound| {
            matches!(&outbound.kind, vpn_domain::client::GraphOutboundKind::Proxy(profile) if wireguard_profile(profile))
        }),
        _ => false,
    }
}
