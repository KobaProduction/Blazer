//! Fetch and commit policy for provider-controlled subscription updates.
use std::fmt;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use vpn_domain::client::{ClientError, ClientSettings, Secret, SubscriptionHttpError};
use vpn_domain::provider::{
    PendingProviderChange, ProviderChangeKind, ProviderEndpoint, ProviderKey, ProviderMetadata,
    ProviderState,
};

use crate::client::{Cancellation, SubscriptionSourcePort, UuidPort};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FetchError {
    Transport,
    Timeout,
    HttpStatus(SubscriptionHttpError),
    Rejected(ClientError),
}

impl From<FetchError> for ClientError {
    fn from(value: FetchError) -> Self {
        match value {
            FetchError::Transport => Self::Network,
            FetchError::HttpStatus(response) => Self::SubscriptionHttp(response),
            FetchError::Timeout => Self::Timeout,
            FetchError::Rejected(error) => error,
        }
    }
}

impl FetchError {
    fn allows_fallback(self) -> bool {
        match self {
            Self::Transport | Self::Timeout => true,
            Self::HttpStatus(response) => (300..=599).contains(&response.status()),
            Self::Rejected(_) => false,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct FetchRequest<'a> {
    pub endpoint: &'a ProviderEndpoint,
    pub timeout: Duration,
    pub follow_same_origin_redirects: bool,
    pub known_version: Option<&'a Secret>,
    pub send_hwid: bool,
    pub user_agent: Option<&'a Secret>,
}

pub struct FetchedSubscription {
    pub body: Vec<u8>,
    pub metadata: ProviderMetadata,
}

pub enum FetchOutcome {
    Modified {
        document: FetchedSubscription,
        version: Option<Secret>,
    },
    Unchanged {
        retain_version: bool,
    },
}

impl fmt::Debug for FetchedSubscription {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FetchedSubscription")
            .field("body_bytes", &self.body.len())
            .field("metadata", &self.metadata)
            .finish()
    }
}

pub(crate) fn fetch_update(
    source: &dyn SubscriptionSourcePort,
    primary: &ProviderEndpoint,
    state: &ProviderState,
    settings: &ClientSettings,
    cancel: &Cancellation,
) -> Result<(FetchOutcome, bool), ClientError> {
    let prior_metadata = state
        .authority_origin
        .as_ref()
        .filter(|origin| *origin == &primary.origin);
    let timeout =
        Duration::from_secs(prior_metadata.map_or(9, |_| state.metadata.request_timeout()));
    let fallback = state.fallback.as_ref().filter(|fallback| {
        !primary.same_request(fallback) && check_security(primary, fallback).is_ok()
    });
    let request = FetchRequest {
        endpoint: primary,
        timeout,
        follow_same_origin_redirects: fallback.is_none(),
        known_version: state.cached_version.as_ref(),
        send_hwid: settings.send_subscription_hwid,
        user_agent: settings.subscription_user_agent.as_ref(),
    };
    match source.fetch(request, cancel) {
        Ok(result) => Ok((result, false)),
        Err(error) if error.allows_fallback() => {
            let fallback = fallback.ok_or(error)?;
            cancel.check()?;
            let request = FetchRequest {
                endpoint: fallback,
                timeout,
                follow_same_origin_redirects: false,
                known_version: None,
                send_hwid: settings.send_subscription_hwid,
                user_agent: settings.subscription_user_agent.as_ref(),
            };
            Ok((source.fetch(request, cancel)?, true))
        }
        Err(error) => Err(error.into()),
    }
}

pub(crate) fn record_success(state: &mut ProviderState, used_fallback: bool) {
    state.last_success_unix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs());
    state.last_automatic_failure_unix = None;
    state.last_update_used_fallback = used_fallback;
}

pub(crate) fn stage_provider(
    source: &dyn SubscriptionSourcePort,
    uuids: &dyn UuidPort,
    primary: ProviderEndpoint,
    mut state: ProviderState,
    incoming: ProviderMetadata,
    used_fallback: bool,
) -> Result<(Secret, ProviderState), ClientError> {
    record_success(&mut state, used_fallback);
    if used_fallback {
        return Ok((primary.url, state));
    }

    incoming.validate()?;
    update_authority(&mut state, &primary);
    // A complete primary document owns the current metadata, including omitted fields.
    state.metadata = incoming;
    update_fallback(source, uuids, &primary, &mut state)?;
    let migration = migration_target(source, &primary, &state.metadata)?;
    let next_primary = stage_endpoint(
        &mut state,
        &primary,
        migration,
        ProviderChangeKind::Migration,
        uuids,
    )?
    .unwrap_or(primary);
    if state
        .fallback
        .as_ref()
        .is_some_and(|fallback| next_primary.same_request(fallback))
    {
        state.fallback = None;
        state.fallback_user_set = false;
    }
    Ok((next_primary.url, state))
}

fn update_fallback(
    source: &dyn SubscriptionSourcePort,
    uuids: &dyn UuidPort,
    primary: &ProviderEndpoint,
    state: &mut ProviderState,
) -> Result<(), ClientError> {
    if state.fallback_user_set {
        state
            .pending
            .retain(|pending| pending.kind != ProviderChangeKind::Fallback);
        return Ok(());
    }
    let target = state
        .metadata
        .value(ProviderKey::FallbackUrl)
        .map(|fallback| source.endpoint(fallback.expose()))
        .transpose()?;
    if target.is_none()
        && state
            .fallback
            .as_ref()
            .is_some_and(|fallback| fallback.origin == primary.origin)
    {
        // Same-origin fallbacks are applied automatically. Cross-origin ones required consent.
        state.fallback = None;
    }
    if let Some(target) =
        stage_endpoint(state, primary, target, ProviderChangeKind::Fallback, uuids)?
    {
        state.fallback = (!primary.same_request(&target)).then_some(target);
    }
    Ok(())
}

fn update_authority(state: &mut ProviderState, primary: &ProviderEndpoint) {
    if state.authority_origin.as_deref() != Some(primary.origin.as_str()) {
        state.authority_origin = Some(primary.origin.clone());
        state.pending.clear();
        if !state.fallback_user_set {
            state.fallback = None;
        }
    }
}

fn migration_target(
    source: &dyn SubscriptionSourcePort,
    primary: &ProviderEndpoint,
    metadata: &ProviderMetadata,
) -> Result<Option<ProviderEndpoint>, ClientError> {
    if let Some(url) = metadata.value(ProviderKey::NewUrl) {
        return source.endpoint(url.expose()).map(Some);
    }
    metadata
        .value(ProviderKey::NewDomain)
        .map(|domain| source.replace_domain(primary, domain.expose()))
        .transpose()
}

fn stage_endpoint(
    state: &mut ProviderState,
    primary: &ProviderEndpoint,
    target: Option<ProviderEndpoint>,
    kind: ProviderChangeKind,
    uuids: &dyn UuidPort,
) -> Result<Option<ProviderEndpoint>, ClientError> {
    let Some(target) = target else {
        state.pending.retain(|pending| pending.kind != kind);
        return Ok(None);
    };
    check_security(primary, &target)?;
    if primary.origin == target.origin {
        state.pending.retain(|pending| pending.kind != kind);
        return Ok(Some(target));
    }
    let committed_fallback =
        kind == ProviderChangeKind::Fallback && state.fallback.as_ref() == Some(&target);
    if committed_fallback {
        state.pending.retain(|pending| pending.kind != kind);
        return Ok(None);
    }
    if !state.pending.iter().any(|pending| {
        pending.kind == kind && pending.from_origin == primary.origin && pending.endpoint == target
    }) {
        state.pending.retain(|pending| pending.kind != kind);
        state.pending.push(PendingProviderChange {
            kind,
            nonce: uuids.new_uuid(),
            from_origin: primary.origin.clone(),
            endpoint: target,
        });
    }
    Ok(None)
}

pub(crate) fn resolve_pending(
    primary: &ProviderEndpoint,
    state: &mut ProviderState,
    nonce: &str,
    approve: bool,
) -> Result<Option<Secret>, ClientError> {
    let index = state
        .pending
        .iter()
        .position(|pending| pending.nonce == nonce && pending.from_origin == primary.origin)
        .ok_or(ClientError::InvalidInput)?;
    let pending = state.pending.remove(index);
    if !approve {
        return Ok(None);
    }
    check_security(primary, &pending.endpoint)?;
    match pending.kind {
        ProviderChangeKind::Fallback => {
            state.fallback = Some(pending.endpoint);
            Ok(None)
        }
        ProviderChangeKind::Migration => {
            // Approval is tied to the previous origin; a new primary does not inherit its proposals.
            state.pending.clear();
            state.cached_version = None;
            if !state.fallback_user_set {
                state.fallback = None;
            }
            if state.fallback.as_ref().is_some_and(|fallback| {
                pending.endpoint.same_request(fallback)
                    || check_security(&pending.endpoint, fallback).is_err()
            }) {
                state.fallback = None;
                state.fallback_user_set = false;
            }
            Ok(Some(pending.endpoint.url))
        }
    }
}

pub fn check_security(
    primary: &ProviderEndpoint,
    target: &ProviderEndpoint,
) -> Result<(), ClientError> {
    if primary.origin.starts_with("https://") && !target.origin.starts_with("https://") {
        return Err(ClientError::InvalidUrl);
    }
    Ok(())
}
