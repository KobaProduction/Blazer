//! Manual import and catalog profile commands.

use super::{
    Catalog, CatalogProfile, ClientApplication, ClientError, MAX_PROFILES, ProxyProfile,
    Subscription,
};

impl ClientApplication {
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
