//! Subscription fetch, refresh and provider-change commands.

use std::collections::HashSet;

use super::{
    Cancellation, Catalog, ClientApplication, ClientError, MAX_SUBSCRIPTIONS,
    NormalizedSubscription, ProfileUnavailableReason, ProxyProfile, Subscription,
    UnavailableProfile,
};
use crate::provider::{
    FetchOutcome, check_security, fetch_update, record_success, resolve_pending, stage_provider,
};
use vpn_domain::client::{
    ProxyTransport, Secret, TcpProbeEndpoint, preserve_profile_uuids,
    validate_subscription_user_agent,
};
use vpn_domain::provider::{ProviderChangeKind, ProviderState};

struct SubscriptionUpdate {
    url: Secret,
    provider: ProviderState,
    profiles: Option<NormalizedSubscription>,
}

impl ClientApplication {
    pub fn add_subscription(
        &mut self,
        title: &str,
        url: &str,
        cancel: &Cancellation,
    ) -> Result<(), ClientError> {
        Subscription::validate_title_override(title)?;
        if self.catalog.subscriptions.len() >= MAX_SUBSCRIPTIONS {
            return Err(ClientError::PayloadTooLarge);
        }
        let update = self.fetch_subscription_update(url, ProviderState::default(), cancel)?;
        let NormalizedSubscription {
            profiles,
            mut unavailable_profiles,
        } = update.profiles.ok_or(ClientError::Network)?;
        self.assign_probe_ids(&mut unavailable_profiles, &[]);
        let auto_title = title.is_empty();
        let title = self.subscription_title(title, &update.url, &update.provider)?;
        let subscription_uuid = self.services.uuids.new_uuid();
        let mut candidate = self.catalog.clone();
        candidate.subscriptions.push(Subscription {
            subscription_uuid: subscription_uuid.clone(),
            title,
            auto_title,
            url: update.url,
            provider: update.provider,
            unavailable_profiles,
        });
        self.append_profiles(&mut candidate, profiles, Some(&subscription_uuid), None)?;
        cancel.check()?;
        self.commit(candidate)
    }

    pub fn refresh_subscription(
        &mut self,
        subscription_uuid: &str,
        cancel: &Cancellation,
    ) -> Result<(), ClientError> {
        let subscription = self.subscription(subscription_uuid)?.clone();
        let title = if subscription.auto_title {
            ""
        } else {
            &subscription.title
        };
        self.update_subscription(subscription_uuid, title, subscription.url.expose(), cancel)
    }

    pub fn record_automatic_refresh_failure(
        &mut self,
        subscription_uuid: &str,
        unix: u64,
        cancel: &Cancellation,
    ) -> Result<(), ClientError> {
        cancel.check()?;
        if self
            .subscription(subscription_uuid)?
            .provider
            .refresh_interval_hours()
            .is_none()
        {
            return Ok(());
        }
        let mut candidate = self.catalog.clone();
        subscription_mut(&mut candidate, subscription_uuid)?
            .provider
            .last_automatic_failure_unix = Some(unix);
        cancel.check()?;
        self.commit(candidate)
    }

    pub fn update_subscription(
        &mut self,
        subscription_uuid: &str,
        title: &str,
        url: &str,
        cancel: &Cancellation,
    ) -> Result<(), ClientError> {
        Subscription::validate_title_override(title)?;
        self.reject_active_subscription(subscription_uuid)?;
        let current = self.subscription(subscription_uuid)?;
        let state = if current.url.expose() == url {
            current.provider.clone()
        } else {
            ProviderState::default()
        };
        let update = self.fetch_subscription_update(url, state, cancel)?;
        let auto_title = title.is_empty();
        let title = self.subscription_title(title, &update.url, &update.provider)?;
        let mut candidate = self.catalog.clone();
        let subscription = subscription_mut(&mut candidate, subscription_uuid)?;
        subscription.auto_title = auto_title;
        subscription.title = title;
        subscription.url = update.url;
        subscription.provider = update.provider;
        if let Some(mut profiles) = update.profiles {
            self.assign_probe_ids(
                &mut profiles.unavailable_profiles,
                &current.unavailable_profiles,
            );
            subscription.unavailable_profiles = profiles.unavailable_profiles;
            self.replace_subscription_profiles(
                &mut candidate,
                subscription_uuid,
                profiles.profiles,
            )?;
        }
        cancel.check()?;
        self.commit(candidate)
    }

    pub fn rename_subscription(
        &mut self,
        subscription_uuid: &str,
        title: &str,
        cancel: &Cancellation,
    ) -> Result<(), ClientError> {
        Subscription::validate_title_override(title)?;
        cancel.check()?;
        let current = self.subscription(subscription_uuid)?;
        let auto_title = title.is_empty();
        let title = self.subscription_title(title, &current.url, &current.provider)?;
        if current.title == title && current.auto_title == auto_title {
            return Ok(());
        }
        let mut candidate = self.catalog.clone();
        let subscription = subscription_mut(&mut candidate, subscription_uuid)?;
        subscription.auto_title = auto_title;
        subscription.title = title;
        cancel.check()?;
        self.commit(candidate)
    }

    fn subscription_title(
        &self,
        manual: &str,
        url: &Secret,
        provider: &ProviderState,
    ) -> Result<String, ClientError> {
        if !manual.is_empty() {
            return Ok(manual.to_owned());
        }
        let source = self.services.source.as_ref();
        if let Some(title) = source.provider_title(url.expose(), &provider.metadata) {
            return Ok(title);
        }
        let endpoint = source.endpoint(url.expose())?;
        let authority = endpoint
            .origin
            .split_once("://")
            .map_or("", |(_, host)| host);
        let host = if authority.starts_with('[') {
            authority.find(']').map_or("", |end| &authority[..=end])
        } else {
            authority.split(':').next().unwrap_or_default()
        };
        Ok(if Subscription::validate_title(host).is_ok() {
            host.to_owned()
        } else {
            "Subscription".to_owned()
        })
    }

    fn assign_probe_ids(&self, rows: &mut [UnavailableProfile], previous: &[UnavailableProfile]) {
        let mut used = HashSet::new();
        for row in rows {
            let Some(endpoint) = row.probe_endpoint() else {
                continue;
            };
            let previous_id = previous
                .iter()
                .filter(|old| {
                    old.protocol == row.protocol && old.probe_endpoint() == Some(endpoint)
                })
                .filter_map(|old| old.probe_id.as_ref())
                .find(|id| used.insert(*id));
            row.probe_id = Some(
                previous_id
                    .cloned()
                    .unwrap_or_else(|| self.services.uuids.new_uuid()),
            );
        }
    }

    fn replace_subscription_profiles(
        &self,
        candidate: &mut Catalog,
        subscription_uuid: &str,
        mut profiles: Vec<ProxyProfile>,
    ) -> Result<(), ClientError> {
        let old_profiles: Vec<_> = candidate
            .profiles
            .iter()
            .filter(|item| item.subscription_uuid.as_deref() == Some(subscription_uuid))
            .map(|item| item.profile.clone())
            .collect();
        candidate
            .profiles
            .retain(|item| item.subscription_uuid.as_deref() != Some(subscription_uuid));
        preserve_profile_uuids(&mut profiles, &old_profiles);
        self.append_profiles(candidate, profiles, Some(subscription_uuid), None)
    }

    fn fetch_subscription_update(
        &self,
        url: &str,
        mut state: ProviderState,
        cancel: &Cancellation,
    ) -> Result<SubscriptionUpdate, ClientError> {
        let source = self.services.source.as_ref();
        let primary = source.endpoint(url)?;
        let (outcome, used_fallback) =
            fetch_update(source, &primary, &state, &self.catalog.settings, cancel)?;
        let (document, version) = match outcome {
            FetchOutcome::Modified { document, version } => (document, version),
            FetchOutcome::Unchanged { retain_version } => {
                if used_fallback || state.cached_version.is_none() {
                    return Err(ClientError::Network);
                }
                record_success(&mut state, false);
                if !retain_version {
                    state.cached_version = None;
                }
                return Ok(SubscriptionUpdate {
                    url: primary.url,
                    provider: state,
                    profiles: None,
                });
            }
        };
        let profiles = self.validate_subscription_profiles(
            self.services
                .normalizer
                .normalize_subscription(&document.body)?,
        )?;
        cancel.check()?;
        let request_url = primary.url.clone();
        let (url, mut state) = stage_provider(
            source,
            self.services.uuids.as_ref(),
            primary,
            state,
            document.metadata,
            used_fallback,
        )?;
        state.cached_version = if !used_fallback && url == request_url {
            version
        } else {
            None
        };
        Ok(SubscriptionUpdate {
            url,
            provider: state,
            profiles: Some(profiles),
        })
    }

    fn validate_subscription_profiles(
        &self,
        mut entries: NormalizedSubscription,
    ) -> Result<NormalizedSubscription, ClientError> {
        for profile in std::mem::take(&mut entries.profiles) {
            match self.services.engine.validate_profile(&profile) {
                Ok(()) => entries.profiles.push(profile),
                Err(error) => {
                    let reason = match error {
                        ClientError::UnsupportedXhttpUploadMethod => {
                            ProfileUnavailableReason::UnsupportedXhttpUploadMethod
                        }
                        ClientError::UnsupportedFeature
                        | ClientError::UnsupportedFormat
                        | ClientError::UnsupportedMode => ProfileUnavailableReason::Unsupported,
                        ClientError::InvalidInput
                        | ClientError::InvalidSubscription
                        | ClientError::EngineInvalid => ProfileUnavailableReason::Invalid,
                        error => return Err(error),
                    };
                    let protocol = profile.protocol.name();
                    let label = if profile.name == format!("{protocol} {}", profile.host) {
                        ""
                    } else {
                        &profile.name
                    };
                    let endpoint = if matches!(profile.transport, ProxyTransport::Quic { .. }) {
                        None
                    } else {
                        TcpProbeEndpoint::new(&profile.host, profile.port)
                    };
                    entries.unavailable_profiles.push(
                        UnavailableProfile::new(label, Some(protocol), reason)
                            .with_probe_endpoint(endpoint),
                    );
                }
            }
        }
        if entries.profiles.is_empty() && entries.unavailable_profiles.is_empty() {
            return Err(ClientError::InvalidSubscription);
        }
        Ok(entries)
    }

    pub fn resolve_provider_change(
        &mut self,
        subscription_uuid: &str,
        nonce: &str,
        approve: bool,
        cancel: &Cancellation,
    ) -> Result<(), ClientError> {
        let mut candidate = self.catalog.clone();
        let subscription = subscription_mut(&mut candidate, subscription_uuid)?;
        let primary = self.services.source.endpoint(subscription.url.expose())?;
        if let Some(url) = resolve_pending(&primary, &mut subscription.provider, nonce, approve)? {
            subscription.url = url;
        }
        cancel.check()?;
        self.commit(candidate)
    }

    pub fn set_subscription_hwid(&mut self, enabled: bool) -> Result<(), ClientError> {
        if self.catalog.settings.send_subscription_hwid == enabled {
            return Ok(());
        }
        let mut candidate = self.catalog.clone();
        candidate.settings.send_subscription_hwid = enabled;
        clear_subscription_validators(&mut candidate);
        self.commit(candidate)
    }

    pub fn set_subscription_user_agent(
        &mut self,
        value: Option<Secret>,
    ) -> Result<(), ClientError> {
        if let Some(value) = &value {
            validate_subscription_user_agent(value.expose())?;
        }
        if self.catalog.settings.subscription_user_agent == value {
            return Ok(());
        }
        let mut candidate = self.catalog.clone();
        candidate.settings.subscription_user_agent = value;
        clear_subscription_validators(&mut candidate);
        self.commit(candidate)
    }

    pub fn set_subscription_fallback(
        &mut self,
        subscription_uuid: &str,
        url: Option<&str>,
        cancel: &Cancellation,
    ) -> Result<(), ClientError> {
        let mut candidate = self.catalog.clone();
        let subscription = subscription_mut(&mut candidate, subscription_uuid)?;
        let source = self.services.source.as_ref();
        let primary = source.endpoint(subscription.url.expose())?;
        let fallback = url.map(|url| source.endpoint(url)).transpose()?;
        if let Some(fallback) = &fallback {
            check_security(&primary, fallback)?;
            if primary.same_request(fallback) {
                return Err(ClientError::InvalidUrl);
            }
        }
        subscription.provider.fallback_user_set = fallback.is_some();
        subscription.provider.fallback = fallback;
        subscription
            .provider
            .pending
            .retain(|pending| pending.kind != ProviderChangeKind::Fallback);
        cancel.check()?;
        self.commit(candidate)
    }

    fn subscription(&self, uuid: &str) -> Result<&Subscription, ClientError> {
        self.catalog
            .subscriptions
            .iter()
            .find(|item| item.subscription_uuid == uuid)
            .ok_or(ClientError::SubscriptionNotFound)
    }

    pub fn remove_subscription(&mut self, subscription_uuid: &str) -> Result<(), ClientError> {
        self.reject_active_subscription(subscription_uuid)?;
        if !self
            .catalog
            .subscriptions
            .iter()
            .any(|item| item.subscription_uuid == subscription_uuid)
        {
            return Err(ClientError::SubscriptionNotFound);
        }
        let mut candidate = self.catalog.clone();
        candidate
            .subscriptions
            .retain(|item| item.subscription_uuid != subscription_uuid);
        candidate.routing.assignments.remove(subscription_uuid);
        candidate
            .profiles
            .retain(|item| item.subscription_uuid.as_deref() != Some(subscription_uuid));
        candidate.repair_selection();
        self.commit(candidate)
    }

    fn reject_active_subscription(&self, subscription_uuid: &str) -> Result<(), ClientError> {
        let active = self.active_profile_uuid();
        if self.catalog.profiles.iter().any(|item| {
            item.subscription_uuid.as_deref() == Some(subscription_uuid)
                && active == Some(item.profile.profile_uuid.as_str())
        }) {
            Err(ClientError::DisconnectFirst)
        } else {
            Ok(())
        }
    }
}

fn clear_subscription_validators(catalog: &mut Catalog) {
    for subscription in &mut catalog.subscriptions {
        subscription.provider.cached_version = None;
    }
}

fn subscription_mut<'a>(
    catalog: &'a mut Catalog,
    uuid: &str,
) -> Result<&'a mut Subscription, ClientError> {
    catalog
        .subscriptions
        .iter_mut()
        .find(|item| item.subscription_uuid == uuid)
        .ok_or(ClientError::SubscriptionNotFound)
}
