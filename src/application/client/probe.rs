use std::collections::HashMap;
use std::time::Duration;

use super::{
    Cancellation, ClientApplication, ClientError, ConnectionMode, ProxyProfile, UnavailableProfile,
};
use crate::session::ProfileLatency;
use vpn_domain::client::{ProbeMethod, ProbeSettings};

type ProbeTarget<'a> = (&'a str, (&'a str, u16));

impl ClientApplication {
    pub fn supports_icmp_probe(&self) -> bool {
        self.services.profile_probe.supports_icmp()
    }

    pub fn set_probe_settings(&mut self, settings: ProbeSettings) -> Result<(), ClientError> {
        settings.validate()?;
        if settings.method == ProbeMethod::Icmp && !self.supports_icmp_probe() {
            return Err(ClientError::UnsupportedFeature);
        }
        if settings == self.catalog.settings.probe {
            return Ok(());
        }
        let mut candidate = self.catalog.clone();
        candidate.settings.probe = settings;
        self.commit(candidate)?;
        self.profile_latencies.clear();
        Ok(())
    }

    pub fn profile_latencies(&self) -> &HashMap<String, ProfileLatency> {
        &self.profile_latencies
    }

    pub fn can_probe_profile(&self) -> bool {
        if self.active_mode() != Some(ConnectionMode::Tun) {
            return true;
        }
        match self.catalog.settings.probe.method {
            ProbeMethod::Tcp => true,
            ProbeMethod::Icmp => false,
            ProbeMethod::Automatic | ProbeMethod::Proxy => {
                self.services.engine.supports_active_tunnel_probe()
            }
        }
    }

    pub fn probe_profile(
        &mut self,
        profile_uuid: &str,
        cancel: &Cancellation,
    ) -> Result<(), ClientError> {
        self.check_probe_allowed(cancel)?;
        let active_tunnel = self.use_active_tunnel_probe();
        if active_tunnel && self.active_profile_uuid() != Some(profile_uuid) {
            return Err(ClientError::DisconnectFirst);
        }
        let results = match self.saved_probe_profile(profile_uuid) {
            Some(profile) if active_tunnel || self.needs_engine_probe(profile) => {
                vec![(
                    profile_uuid.to_owned(),
                    self.measure_tunnel(profile, cancel)?,
                )]
            }
            _ => self.measure_targets(&[self.probe_target(profile_uuid)?], cancel)?,
        };
        cancel.check()?;
        self.profile_latencies.extend(results);
        Ok(())
    }

    fn saved_probe_profile(&self, profile_uuid: &str) -> Option<&ProxyProfile> {
        self.catalog
            .profiles
            .iter()
            .find(|item| item.profile.profile_uuid == profile_uuid)
            .map(|item| &item.profile)
    }

    fn probe_target(&self, profile_uuid: &str) -> Result<ProbeTarget<'_>, ClientError> {
        if let Some(profile) = self.saved_probe_profile(profile_uuid) {
            return supported_target(profile, self.catalog.settings.probe.method)
                .ok_or(ClientError::UnsupportedFeature);
        }
        self.unavailable_probe_targets(None)
            .find(|(uuid, _)| *uuid == profile_uuid)
            .ok_or(ClientError::ProfileNotFound)
    }

    pub fn probe_profiles(
        &mut self,
        subscription_uuid: Option<&str>,
        cancel: &Cancellation,
    ) -> Result<(), ClientError> {
        self.check_probe_allowed(cancel)?;
        if let Some(uuid) = subscription_uuid
            && !self
                .catalog
                .subscriptions
                .iter()
                .any(|item| item.subscription_uuid == uuid)
        {
            return Err(ClientError::SubscriptionNotFound);
        }
        if self.use_active_tunnel_probe() {
            return self.probe_active_tunnel_profiles(subscription_uuid, cancel);
        }
        let profiles = self
            .catalog
            .profiles
            .iter()
            .filter(|item| {
                subscription_uuid.is_none_or(|uuid| item.subscription_uuid.as_deref() == Some(uuid))
            })
            .map(|item| &item.profile);
        let targets: Vec<_> = profiles
            .clone()
            .filter_map(|profile| supported_target(profile, self.catalog.settings.probe.method))
            .chain(self.unavailable_probe_targets(subscription_uuid))
            .collect();
        let mut results = if targets.is_empty() {
            Vec::new()
        } else {
            self.measure_targets(&targets, cancel)?
        };
        self.measure_tunnels(
            profiles.filter(|profile| self.needs_engine_probe(profile)),
            cancel,
            &mut results,
        )?;
        if results.is_empty() {
            return Err(ClientError::UnsupportedFeature);
        }
        cancel.check()?;
        self.profile_latencies.extend(results);
        Ok(())
    }

    fn probe_active_tunnel_profiles(
        &mut self,
        subscription_uuid: Option<&str>,
        cancel: &Cancellation,
    ) -> Result<(), ClientError> {
        let profiles = self.catalog.profiles.iter().filter(|item| {
            subscription_uuid.is_none_or(|uuid| item.subscription_uuid.as_deref() == Some(uuid))
        });
        let mut results = Vec::new();
        self.measure_tunnels(profiles.map(|item| &item.profile), cancel, &mut results)?;
        results.extend(
            self.unavailable_probe_targets(subscription_uuid)
                .map(|(uuid, _)| {
                    (
                        uuid.to_owned(),
                        ProfileLatency::Failed(ClientError::DisconnectFirst),
                    )
                }),
        );
        if results.is_empty() {
            return Err(ClientError::UnsupportedFeature);
        }
        cancel.check()?;
        self.profile_latencies.extend(results);
        Ok(())
    }

    fn measure_tunnels<'a>(
        &self,
        profiles: impl Iterator<Item = &'a ProxyProfile>,
        cancel: &Cancellation,
        results: &mut Vec<(String, ProfileLatency)>,
    ) -> Result<(), ClientError> {
        for profile in profiles {
            cancel.check()?;
            let result = match self.measure_tunnel(profile, cancel) {
                Ok(time) => time,
                Err(ClientError::Cancelled) => return Err(ClientError::Cancelled),
                Err(error) => ProfileLatency::Failed(error),
            };
            results.push((profile.profile_uuid.clone(), result));
        }
        Ok(())
    }

    fn measure_tunnel(
        &self,
        profile: &ProxyProfile,
        cancel: &Cancellation,
    ) -> Result<ProfileLatency, ClientError> {
        let active_tunnel = self.active_mode() == Some(ConnectionMode::Tun);
        if active_tunnel && self.active_profile_uuid() != Some(profile.profile_uuid.as_str()) {
            return Err(ClientError::DisconnectFirst);
        }
        let supported = if active_tunnel {
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
        let reuse_active_proxy = !active_tunnel
            && self
                .active
                .as_ref()
                .is_some_and(|connection| connection.routing.is_none());
        match self.services.engine.probe_tunnel(
            profile,
            active,
            reuse_active_proxy,
            &self.catalog.settings.probe,
            cancel,
        ) {
            Err(ClientError::Network) => Ok(ProfileLatency::CheckUnreachable),
            result => latency(result),
        }
    }

    fn unavailable_probe_targets<'a>(
        &'a self,
        subscription_uuid: Option<&'a str>,
    ) -> impl Iterator<Item = ProbeTarget<'a>> {
        self.catalog
            .subscriptions
            .iter()
            .filter(move |item| subscription_uuid.is_none_or(|uuid| item.subscription_uuid == uuid))
            .flat_map(|item| &item.unavailable_profiles)
            .filter_map(unavailable_target)
            .filter(|_| self.catalog.settings.probe.method != ProbeMethod::Proxy)
    }

    fn check_probe_allowed(&self, cancel: &Cancellation) -> Result<(), ClientError> {
        cancel.check()?;
        if !self.can_probe_profile() {
            return Err(ClientError::UnsupportedMode);
        }
        Ok(())
    }

    fn measure_targets(
        &self,
        targets: &[ProbeTarget<'_>],
        cancel: &Cancellation,
    ) -> Result<Vec<(String, ProfileLatency)>, ClientError> {
        if targets.is_empty() {
            return Err(ClientError::UnsupportedFeature);
        }
        let endpoints: Vec<_> = targets.iter().map(|(_, address)| *address).collect();
        let results = self.services.profile_probe.connect_times(
            &endpoints,
            &self.catalog.settings.probe,
            cancel,
        )?;
        targets
            .iter()
            .zip(results)
            .map(|((uuid, _), result)| Ok(((*uuid).to_owned(), latency(result)?)))
            .collect()
    }

    fn needs_engine_probe(&self, profile: &ProxyProfile) -> bool {
        match self.catalog.settings.probe.method {
            ProbeMethod::Proxy => true,
            ProbeMethod::Automatic => needs_tunnel_probe(profile),
            ProbeMethod::Tcp | ProbeMethod::Icmp => false,
        }
    }

    fn use_active_tunnel_probe(&self) -> bool {
        self.active_mode() == Some(ConnectionMode::Tun)
            && matches!(
                self.catalog.settings.probe.method,
                ProbeMethod::Automatic | ProbeMethod::Proxy
            )
    }
}

fn supported_target(profile: &ProxyProfile, method: ProbeMethod) -> Option<ProbeTarget<'_>> {
    if matches!(
        profile.protocol,
        vpn_domain::client::ProxyProtocol::Graph(_)
    ) {
        return None;
    }
    let direct = match method {
        ProbeMethod::Icmp => true,
        ProbeMethod::Automatic | ProbeMethod::Tcp => !needs_tunnel_probe(profile),
        ProbeMethod::Proxy => false,
    };
    direct.then_some((&profile.profile_uuid, (profile.host.as_str(), profile.port)))
}

fn needs_tunnel_probe(profile: &ProxyProfile) -> bool {
    matches!(
        profile.protocol,
        vpn_domain::client::ProxyProtocol::Graph(_)
    ) || !profile.transport.has_tcp_endpoint()
        || matches!((&profile.transport, &profile.security),
            (vpn_domain::client::ProxyTransport::Xhttp { .. }, vpn_domain::client::ProxySecurity::Tls { alpn, .. })
                if alpn.as_slice() == ["h3"])
}

fn unavailable_target(profile: &UnavailableProfile) -> Option<ProbeTarget<'_>> {
    Some((
        profile.probe_id.as_deref()?,
        profile.probe_endpoint()?.address(),
    ))
}

fn latency(result: Result<Duration, ClientError>) -> Result<ProfileLatency, ClientError> {
    match result {
        Ok(elapsed) => Ok(ProfileLatency::Reachable {
            milliseconds: u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
        }),
        Err(ClientError::Timeout) => Ok(ProfileLatency::TimedOut),
        Err(ClientError::Network) => Ok(ProfileLatency::Unreachable),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vpn_domain::client::{
        GraphGroup, GraphOutbound, GraphOutboundKind, GraphStrategy, GraphTarget, ProxyGraph,
        ProxyProtocol, ProxySecurity, ProxyTransport,
    };

    #[test]
    fn graph_probe_never_uses_its_representative_tcp_endpoint() {
        let mut profile = ProxyProfile {
            profile_uuid: "graph".into(),
            name: "Balanced".into(),
            source: None,
            host: "127.0.0.1".into(),
            port: 1080,
            protocol: ProxyProtocol::Socks5 {
                username: None,
                password: None,
            },
            transport: ProxyTransport::Tcp,
            security: ProxySecurity::None,
        };
        let graph = ProxyGraph {
            outbounds: vec![GraphOutbound {
                tag: "first".into(),
                kind: GraphOutboundKind::Proxy(Box::new(profile.clone())),
            }],
            groups: vec![GraphGroup {
                tag: "balanced".into(),
                members: vec!["first".into()],
                strategy: GraphStrategy::RoundRobin,
            }],
            rules: vec![],
            default_target: GraphTarget::Group("balanced".into()),
            dns: vpn_domain::routing::DnsPolicy::Automatic,
            observatory: None,
        };
        graph.validate().unwrap();
        profile.protocol = ProxyProtocol::Graph(Box::new(graph));
        assert!(needs_tunnel_probe(&profile));
        for method in [
            ProbeMethod::Automatic,
            ProbeMethod::Proxy,
            ProbeMethod::Tcp,
            ProbeMethod::Icmp,
        ] {
            assert!(supported_target(&profile, method).is_none());
        }
    }
}
