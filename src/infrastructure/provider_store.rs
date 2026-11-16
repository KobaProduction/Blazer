//! Provider payload inside the encrypted, versioned catalog.
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use vpn_application::provider::check_security;
use vpn_domain::client::{ClientError, Secret};
use vpn_domain::provider::{PendingProviderChange, ProviderChangeKind, ProviderState};

use crate::provider::{endpoint, stored_metadata, valid_etag};

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StoredProviderState {
    #[serde(default)]
    values: BTreeMap<String, String>,
    authority_origin: Option<String>,
    fallback: Option<String>,
    #[serde(default)]
    fallback_user_set: bool,
    #[serde(default)]
    pending: Vec<StoredPending>,
    last_success_unix: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_automatic_failure_unix: Option<u64>,
    #[serde(default)]
    last_update_used_fallback: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cached_version: Option<String>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredPending {
    kind: StoredKind,
    nonce: String,
    from_origin: String,
    url: String,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum StoredKind {
    Fallback,
    Migration,
}

impl StoredProviderState {
    pub fn from_state(state: &ProviderState) -> Self {
        Self {
            values: state
                .metadata
                .values
                .iter()
                .map(|(key, value)| (key.wire_name().to_owned(), value.expose().to_owned()))
                .collect(),
            authority_origin: state.authority_origin.clone(),
            fallback: state
                .fallback
                .as_ref()
                .map(|endpoint| endpoint.url.expose().to_owned()),
            fallback_user_set: state.fallback_user_set,
            pending: state
                .pending
                .iter()
                .map(|pending| StoredPending {
                    kind: match pending.kind {
                        ProviderChangeKind::Fallback => StoredKind::Fallback,
                        ProviderChangeKind::Migration => StoredKind::Migration,
                    },
                    nonce: pending.nonce.clone(),
                    from_origin: pending.from_origin.clone(),
                    url: pending.endpoint.url.expose().to_owned(),
                })
                .collect(),
            last_success_unix: state.last_success_unix,
            last_automatic_failure_unix: state.last_automatic_failure_unix,
            last_update_used_fallback: state.last_update_used_fallback,
            cached_version: state
                .cached_version
                .as_ref()
                .map(|value| value.expose().to_owned()),
        }
    }

    pub fn into_state(self, primary_url: &str) -> Result<ProviderState, ClientError> {
        let primary = endpoint(primary_url)?;
        let metadata = stored_metadata(self.values)?;
        if let Some(origin) = &self.authority_origin
            && endpoint(origin)?.origin != *origin
        {
            return Err(ClientError::StorageCorrupt);
        }
        let fallback = self.fallback.as_deref().map(endpoint).transpose()?;
        if let Some(fallback) = &fallback {
            check_security(&primary, fallback)?;
            if primary.same_request(fallback) {
                return Err(ClientError::StorageCorrupt);
            }
        }
        let pending = self
            .pending
            .into_iter()
            .map(|pending| {
                let target = endpoint(&pending.url)?;
                check_security(&primary, &target)?;
                if pending.from_origin != primary.origin
                    || target.origin == primary.origin
                    || uuid::Uuid::parse_str(&pending.nonce).is_err()
                {
                    return Err(ClientError::StorageCorrupt);
                }
                Ok(PendingProviderChange {
                    kind: match pending.kind {
                        StoredKind::Fallback => ProviderChangeKind::Fallback,
                        StoredKind::Migration => ProviderChangeKind::Migration,
                    },
                    nonce: pending.nonce,
                    from_origin: pending.from_origin,
                    endpoint: target,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        if pending.len() > 2
            || matches!(pending.as_slice(), [left, right]
            if left.kind == right.kind || left.nonce == right.nonce)
        {
            return Err(ClientError::StorageCorrupt);
        }
        Ok(ProviderState {
            metadata,
            authority_origin: self.authority_origin,
            fallback,
            fallback_user_set: self.fallback_user_set,
            pending,
            last_success_unix: self.last_success_unix,
            last_automatic_failure_unix: self.last_automatic_failure_unix,
            last_update_used_fallback: self.last_update_used_fallback,
            cached_version: self
                .cached_version
                .filter(|tag| valid_etag(tag))
                .map(Secret::new),
        })
    }
}
