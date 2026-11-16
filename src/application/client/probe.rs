use std::collections::HashMap;
use std::time::Duration;

use super::{
    Cancellation, ClientApplication, ClientError, ConnectionMode, ProxyProfile, UnavailableProfile,
};
use crate::session::ProfileLatency;

type ProbeTarget<'a> = (&'a str, (&'a str, u16));

impl ClientApplication {
    pub fn profile_latencies(&self) -> &HashMap<String, ProfileLatency> {
        &self.profile_latencies
    }

    pub fn can_probe_profile(&self) -> bool {
        self.active_mode() != Some(ConnectionMode::Tun)
    }

    pub fn probe_profile(
        &mut self,
        profile_uuid: &str,
        cancel: &Cancellation,
    ) -> Result<(), ClientError> {
        self.check_probe_allowed(cancel)?;
        let target = self
            .catalog
            .profiles
            .iter()
            .map(|item| supported_target(&item.profile))
            .chain(self.unavailable_probe_targets(None))
            .find(|(uuid, _)| *uuid == profile_uuid)
            .ok_or(ClientError::ProfileNotFound)?;
        let results = self.measure_targets(&[target], cancel)?;
        cancel.check()?;
        self.profile_latencies.extend(results);
        Ok(())
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
        let targets: Vec<_> = self
            .catalog
            .profiles
            .iter()
            .filter(|item| {
                subscription_uuid.is_none_or(|uuid| item.subscription_uuid.as_deref() == Some(uuid))
            })
            .map(|item| supported_target(&item.profile))
            .chain(self.unavailable_probe_targets(subscription_uuid))
            .collect();
        let results = self.measure_targets(&targets, cancel)?;
        cancel.check()?;
        self.profile_latencies.extend(results);
        Ok(())
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
        let results = self
            .services
            .profile_probe
            .connect_times(&endpoints, cancel)?;
        targets
            .iter()
            .zip(results)
            .map(|((uuid, _), result)| Ok(((*uuid).to_owned(), latency(result)?)))
            .collect()
    }
}

fn supported_target(profile: &ProxyProfile) -> ProbeTarget<'_> {
    (&profile.profile_uuid, (profile.host.as_str(), profile.port))
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
