//! Manual import and catalog profile commands.

use super::{
    Cancellation, Catalog, CatalogProfile, ClientApplication, ClientError, MAX_PROFILES,
    ProxyProfile, Subscription,
};

impl ClientApplication {
    pub fn update_profile(
        &mut self,
        profile_uuid: &str,
        input: &[u8],
        cancel: &Cancellation,
    ) -> Result<(), ClientError> {
        cancel.check()?;
        if self.active_profile_uuid() == Some(profile_uuid) {
            return Err(ClientError::DisconnectFirst);
        }
        let index = self
            .catalog
            .profiles
            .iter()
            .position(|item| item.profile.profile_uuid == profile_uuid)
            .ok_or(ClientError::ProfileNotFound)?;
        if self.profile_source_hidden(&self.catalog.profiles[index]) {
            return Err(ClientError::UnsupportedFeature);
        }
        let [mut profile]: [ProxyProfile; 1] = self
            .normalize_profiles(input)?
            .try_into()
            .map_err(|_| ClientError::InvalidSubscription)?;
        profile.profile_uuid = profile_uuid.to_owned();
        let mut candidate = self.catalog.clone();
        candidate.profiles[index].profile = profile;
        cancel.check()?;
        self.commit(candidate)?;
        self.profile_latencies.remove(profile_uuid);
        Ok(())
    }

    pub fn profile_source_hidden(&self, profile: &CatalogProfile) -> bool {
        profile.subscription_uuid.as_deref().is_some_and(|uuid| {
            self.catalog.subscriptions.iter().any(|subscription| {
                subscription.subscription_uuid == uuid && subscription.provider.raw_config_hidden()
            })
        })
    }

    pub fn import_text(&mut self, title: &str, input: &[u8]) -> Result<(), ClientError> {
        Subscription::validate_title(title)?;
        if input.len() > crate::MAX_SUBSCRIPTION_BYTES {
            return Err(ClientError::PayloadTooLarge);
        }
        let profiles = self.normalize_profiles(input)?;
        if profiles.is_empty() {
            return Err(ClientError::InvalidSubscription);
        }
        let mut candidate = self.catalog.clone();
        self.append_profiles(&mut candidate, profiles, None, Some(title))?;
        self.commit(candidate)
    }

    pub(super) fn normalize_profiles(
        &self,
        input: &[u8],
    ) -> Result<Vec<ProxyProfile>, ClientError> {
        let profiles = self.services.normalizer.normalize(input)?;
        for profile in &profiles {
            self.services.engine.validate_profile(profile)?;
        }
        Ok(profiles)
    }

    pub(super) fn append_profiles(
        &self,
        candidate: &mut Catalog,
        profiles: Vec<ProxyProfile>,
        subscription_uuid: Option<&str>,
        title: Option<&str>,
    ) -> Result<(), ClientError> {
        if candidate.profile_count().saturating_add(profiles.len()) > MAX_PROFILES {
            return Err(ClientError::PayloadTooLarge);
        }
        for mut profile in profiles {
            if profile.profile_uuid.is_empty() {
                profile.profile_uuid = self.services.uuids.new_uuid();
            }
            if let Some(title) = title.filter(|_| profile.name.is_empty()) {
                profile.name = title.to_owned();
            }
            candidate.profiles.push(CatalogProfile {
                profile,
                subscription_uuid: subscription_uuid.map(str::to_owned),
            });
        }
        candidate.repair_selection();
        Ok(())
    }

    pub fn select_profile(&mut self, profile_uuid: &str) -> Result<(), ClientError> {
        if self.active.is_some() {
            return Err(ClientError::DisconnectFirst);
        }
        if !self
            .catalog
            .profiles
            .iter()
            .any(|item| item.profile.profile_uuid == profile_uuid)
        {
            return Err(ClientError::ProfileNotFound);
        }
        let mut candidate = self.catalog.clone();
        candidate.selected_profile_uuid = Some(profile_uuid.to_owned());
        self.commit(candidate)
    }

    pub fn remove_profile(&mut self, profile_uuid: &str) -> Result<(), ClientError> {
        if self.active_profile_uuid() == Some(profile_uuid) {
            return Err(ClientError::DisconnectFirst);
        }
        let profile = self
            .catalog
            .profiles
            .iter()
            .find(|item| item.profile.profile_uuid == profile_uuid)
            .ok_or(ClientError::ProfileNotFound)?;
        let mut candidate = self.catalog.clone();
        if let Some(uuid) = &profile.subscription_uuid
            && let Some(subscription) = candidate
                .subscriptions
                .iter_mut()
                .find(|item| &item.subscription_uuid == uuid)
        {
            subscription.provider.cached_version = None;
        }
        candidate
            .profiles
            .retain(|item| item.profile.profile_uuid != profile_uuid);
        candidate.repair_selection();
        self.commit(candidate)
    }
}
