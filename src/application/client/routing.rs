//! Commands that update the saved policy without changing a running engine.

use crate::session::{
    DnsPolicy, RoutingAssignment, RoutingInput, RoutingProfile, RoutingReconnect, RoutingSelection,
    RoutingSnapshot,
};

use super::{ActiveConnection, Cancellation, ClientApplication, ClientError, ConnectionMode};

impl ClientApplication {
    pub fn routing_snapshot(&self) -> RoutingSnapshot {
        let source = self
            .catalog
            .selected_profile_uuid
            .as_deref()
            .and_then(|uuid| {
                self.catalog
                    .profiles
                    .iter()
                    .find(|item| item.profile.profile_uuid == uuid)
                    .and_then(|item| item.subscription_uuid.as_deref())
            });
        let active_routing = self.routing_for_active();
        RoutingSnapshot {
            saved: self
                .catalog
                .routing
                .effective(source)
                .map(RoutingSelection::from),
            saved_for_active: active_routing
                .and_then(|(_, target)| target)
                .map(RoutingSelection::from),
            active: self
                .active
                .as_ref()
                .and_then(|active| active.routing.as_ref())
                .map(RoutingSelection::from),
            pending_reconnect: active_routing
                .and_then(|(active, target)| self.pending_routing_change(active, target)),
            local_proxy_available: self
                .services
                .engine
                .supports_routing(ConnectionMode::LocalProxy),
            tun_available: self.services.engine.supports_routing(ConnectionMode::Tun),
            last_saved_profile_uuid: self.last_saved_routing_uuid.clone(),
            catalog: self.catalog.routing.clone(),
        }
    }

    pub(super) fn routing_reconnect_pending(&self) -> Option<RoutingReconnect> {
        let (active, target) = self.routing_for_active()?;
        self.pending_routing_change(active, target)
    }

    fn routing_for_active(&self) -> Option<(&ActiveConnection, Option<&RoutingProfile>)> {
        let active = self.active.as_ref()?;
        let profile = self
            .catalog
            .profiles
            .iter()
            .find(|item| item.profile.profile_uuid == active.profile_uuid)?;
        let target = self
            .catalog
            .routing
            .effective(profile.subscription_uuid.as_deref());
        Some((active, target))
    }

    fn pending_routing_change(
        &self,
        active: &ActiveConnection,
        target: Option<&RoutingProfile>,
    ) -> Option<RoutingReconnect> {
        if !self.services.engine.supports_routing(active.mode)
            || !self.capabilities().supports(active.mode)
        {
            return None;
        }
        let equal = match (active.routing.as_ref(), target) {
            (Some(applied), Some(saved)) => {
                applied.uuid == saved.uuid && same_runtime_policy(&applied.input, &saved.input)
            }
            (None, None) => true,
            _ => false,
        };
        (!equal).then(|| RoutingReconnect {
            connection_id: active.connection_id.clone(),
            target: target.map(RoutingSelection::from),
        })
    }

    pub fn save_routing_profile(
        &mut self,
        uuid: Option<String>,
        input: RoutingInput,
        cancel: &Cancellation,
    ) -> Result<(), ClientError> {
        input.validate()?;
        cancel.check()?;
        let profile = self.routing_profile_update(uuid, input)?;
        let uuid = profile.uuid.clone();
        if self.catalog.routing.lookup(&uuid) != Some(&profile) {
            let mut candidate = self.catalog.clone();
            let profiles = &mut candidate.routing.profiles;
            if let Some(existing) = profiles.iter_mut().find(|existing| existing.uuid == uuid) {
                *existing = profile;
            } else {
                profiles.push(profile);
            }
            cancel.check()?;
            self.commit(candidate)?;
        }
        self.last_saved_routing_uuid = Some(uuid);
        Ok(())
    }

    fn routing_profile_update(
        &self,
        uuid: Option<String>,
        input: RoutingInput,
    ) -> Result<RoutingProfile, ClientError> {
        let Some(uuid) = uuid else {
            let uuid = self.services.uuids.new_uuid();
            if self.catalog.routing.lookup(&uuid).is_some() {
                return Err(ClientError::StorageWrite);
            }
            return Ok(RoutingProfile {
                uuid,
                revision: 1,
                input,
            });
        };
        let existing = self
            .catalog
            .routing
            .lookup(&uuid)
            .ok_or(ClientError::RoutingProfileMissing)?;
        let policy_changed = !same_runtime_policy(&existing.input, &input);
        let revision = existing
            .revision
            .checked_add(u64::from(policy_changed))
            .ok_or(ClientError::StorageWrite)?;
        Ok(RoutingProfile {
            uuid,
            revision,
            input,
        })
    }

    pub fn remove_routing_profile(&mut self, uuid: &str) -> Result<(), ClientError> {
        if self.catalog.routing.lookup(uuid).is_none() {
            return Err(ClientError::RoutingProfileMissing);
        }
        let active = self
            .active
            .as_ref()
            .and_then(|active| active.routing.as_ref());
        if self.catalog.routing.is_assigned(uuid)
            || active.is_some_and(|profile| profile.uuid == uuid)
        {
            return Err(ClientError::RoutingProfileAssigned);
        }
        let mut candidate = self.catalog.clone();
        candidate
            .routing
            .profiles
            .retain(|profile| profile.uuid != uuid);
        self.commit(candidate)?;
        if self.last_saved_routing_uuid.as_deref() == Some(uuid) {
            self.last_saved_routing_uuid = None;
        }
        Ok(())
    }

    pub fn set_routing_assignment(
        &mut self,
        subscription_uuid: Option<String>,
        assignment: RoutingAssignment,
    ) -> Result<(), ClientError> {
        if let Some(uuid) = &assignment.profile_uuid
            && self.catalog.routing.lookup(uuid).is_none()
        {
            return Err(ClientError::RoutingProfileMissing);
        }
        let mut candidate = self.catalog.clone();
        match subscription_uuid {
            None => {
                candidate.routing.default_profile_uuid =
                    assignment.profile_uuid.filter(|_| assignment.enabled);
            }
            Some(uuid) => {
                if !candidate
                    .subscriptions
                    .iter()
                    .any(|subscription| subscription.subscription_uuid == uuid)
                {
                    return Err(ClientError::SubscriptionNotFound);
                }
                if assignment == RoutingAssignment::default() {
                    candidate.routing.assignments.remove(&uuid);
                } else {
                    candidate.routing.assignments.insert(uuid, assignment);
                }
            }
        }
        if candidate.routing == self.catalog.routing {
            return Ok(());
        }
        self.commit(candidate)
    }

    pub fn set_dns_policy(
        &mut self,
        uuid: String,
        policy: DnsPolicy,
        cancel: &Cancellation,
    ) -> Result<(), ClientError> {
        let mut input = self
            .catalog
            .routing
            .lookup(&uuid)
            .ok_or(ClientError::RoutingProfileMissing)?
            .input
            .clone();
        input.dns = policy;
        self.save_routing_profile(Some(uuid), input, cancel)
    }
}

fn same_runtime_policy(left: &RoutingInput, right: &RoutingInput) -> bool {
    left.rules == right.rules && left.dns == right.dns
}
