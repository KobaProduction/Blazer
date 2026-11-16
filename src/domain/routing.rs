//! User-owned routing policy and its catalog invariants.

use std::collections::{BTreeMap, HashSet};
use std::net::{IpAddr, SocketAddr};

use crate::client::{ClientError, Subscription};

const MAX_ROUTING_PROFILES: usize = 64;
const MAX_ROUTE_RULES: usize = 256;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RoutingCatalog {
    pub profiles: Vec<RoutingProfile>,
    pub default_profile_uuid: Option<String>,
    pub assignments: BTreeMap<String, RoutingAssignment>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutingProfile {
    pub uuid: String,
    pub revision: u64,
    pub input: RoutingInput,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RoutingInput {
    pub title: String,
    pub rules: Vec<RouteRule>,
    pub dns: DnsPolicy,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RoutingAssignment {
    pub profile_uuid: Option<String>,
    pub enabled: bool,
    pub ignore_provider: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteRule {
    pub target: RouteTarget,
    pub action: RouteAction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteTarget {
    Domain(String),
    DomainSuffix(String),
    Cidr(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteAction {
    Proxy,
    Direct,
    Block,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum DnsPolicy {
    #[default]
    Automatic,
    System,
    Tcp {
        endpoint: SocketAddr,
    },
    Tls {
        endpoint: SocketAddr,
        server_name: String,
    },
    Https {
        endpoint: SocketAddr,
        server_name: String,
        path: String,
    },
}

impl RoutingCatalog {
    pub fn lookup(&self, uuid: &str) -> Option<&RoutingProfile> {
        self.profiles.iter().find(|profile| profile.uuid == uuid)
    }

    pub fn effective(&self, subscription_uuid: Option<&str>) -> Option<&RoutingProfile> {
        let assigned = subscription_uuid
            .and_then(|uuid| self.assignments.get(uuid))
            .filter(|assignment| assignment.enabled)
            .and_then(|assignment| assignment.profile_uuid.as_deref());
        assigned
            .or(self.default_profile_uuid.as_deref())
            .and_then(|uuid| self.lookup(uuid))
    }

    pub fn is_assigned(&self, uuid: &str) -> bool {
        self.default_profile_uuid.as_deref() == Some(uuid)
            || self
                .assignments
                .values()
                .any(|assignment| assignment.profile_uuid.as_deref() == Some(uuid))
    }

    pub(crate) fn validate(&self, subscriptions: &[Subscription]) -> Result<(), ClientError> {
        if self.profiles.len() > MAX_ROUTING_PROFILES {
            return Err(ClientError::PayloadTooLarge);
        }
        let mut uuids = HashSet::with_capacity(self.profiles.len());
        for profile in &self.profiles {
            if profile.uuid.is_empty()
                || profile.uuid.len() > 64
                || profile.revision == 0
                || !uuids.insert(profile.uuid.as_str())
            {
                return Err(ClientError::InvalidInput);
            }
            profile.input.validate()?;
        }
        if self
            .default_profile_uuid
            .as_deref()
            .is_some_and(|uuid| !uuids.contains(uuid))
            || self.assignments.iter().any(|(source, assignment)| {
                !subscriptions
                    .iter()
                    .any(|subscription| &subscription.subscription_uuid == source)
                    || assignment
                        .profile_uuid
                        .as_deref()
                        .is_some_and(|uuid| !uuids.contains(uuid))
            })
        {
            return Err(ClientError::InvalidInput);
        }
        Ok(())
    }
}

impl RoutingInput {
    pub fn validate(&self) -> Result<(), ClientError> {
        if self.title.trim().is_empty()
            || self.title.chars().count() > 127
            || self.title.chars().any(char::is_control)
        {
            return Err(ClientError::InvalidInput);
        }
        if self.rules.len() > MAX_ROUTE_RULES {
            return Err(ClientError::PayloadTooLarge);
        }
        for rule in &self.rules {
            rule.target.validate()?;
        }
        self.dns.validate()
    }
}

impl RouteTarget {
    fn validate(&self) -> Result<(), ClientError> {
        let valid = match self {
            Self::Domain(name) | Self::DomainSuffix(name) => valid_domain(name),
            Self::Cidr(cidr) => valid_cidr(cidr),
        };
        valid.then_some(()).ok_or(ClientError::InvalidInput)
    }
}

impl DnsPolicy {
    pub fn validate(&self) -> Result<(), ClientError> {
        let valid = match self {
            Self::Automatic | Self::System => true,
            Self::Tcp { endpoint } => valid_endpoint(*endpoint),
            Self::Tls {
                endpoint,
                server_name,
            } => valid_endpoint(*endpoint) && valid_domain(server_name),
            Self::Https {
                endpoint,
                server_name,
                path,
            } => valid_endpoint(*endpoint) && valid_domain(server_name) && valid_dns_path(path),
        };
        valid.then_some(()).ok_or(ClientError::InvalidInput)
    }
}

fn valid_endpoint(endpoint: SocketAddr) -> bool {
    if endpoint.port() == 0 || endpoint.ip().is_unspecified() || endpoint.ip().is_multicast() {
        return false;
    }
    match endpoint {
        SocketAddr::V4(endpoint) => !endpoint.ip().is_broadcast(),
        SocketAddr::V6(endpoint) => endpoint.scope_id() == 0 && endpoint.flowinfo() == 0,
    }
}

fn valid_dns_path(path: &str) -> bool {
    path.starts_with('/')
        && !path.starts_with("//")
        && path.len() <= 512
        && !path.split('/').any(|part| matches!(part, "." | ".."))
        && path
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"/-._~".contains(&byte))
}

fn valid_domain(name: &str) -> bool {
    let name = name.strip_suffix('.').unwrap_or(name);
    !name.is_empty()
        && name.len() <= 253
        && name.parse::<IpAddr>().is_err()
        && name.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
}

fn valid_cidr(value: &str) -> bool {
    let Some((address, prefix)) = value.split_once('/') else {
        return false;
    };
    if !prefix.bytes().all(|byte| byte.is_ascii_digit()) {
        return false;
    }
    let (Ok(address), Ok(prefix)) = (address.parse::<IpAddr>(), prefix.parse::<u32>()) else {
        return false;
    };
    match address {
        IpAddr::V4(address) => prefix <= 32 && u32::from(address).trailing_zeros() >= 32 - prefix,
        IpAddr::V6(address) => {
            prefix <= 128 && u128::from(address).trailing_zeros() >= 128 - prefix
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cidrs_require_canonical_network_addresses_and_family_prefixes() {
        for valid in [
            "0.0.0.0/0",
            "192.0.2.0/24",
            "192.0.2.1/32",
            "::/0",
            "2001:db8::/32",
            "::1/128",
        ] {
            assert!(valid_cidr(valid), "{valid}");
        }
        for invalid in [
            "192.0.2.1/24",
            "::1/0",
            "127.0.0.1/33",
            "2001:db8::/129",
            "192.0.2.0/+24",
        ] {
            assert!(!valid_cidr(invalid), "{invalid}");
        }
    }

    #[test]
    fn effective_policy_prefers_enabled_subscription_assignment() {
        let profile = |uuid: &str| RoutingProfile {
            uuid: uuid.into(),
            revision: 1,
            input: RoutingInput {
                title: uuid.into(),
                ..RoutingInput::default()
            },
        };
        let mut routing = RoutingCatalog {
            profiles: vec![profile("global"), profile("assigned")],
            default_profile_uuid: Some("global".into()),
            assignments: BTreeMap::from([(
                "subscription".into(),
                RoutingAssignment {
                    enabled: true,
                    profile_uuid: Some("assigned".into()),
                    ignore_provider: false,
                },
            )]),
        };
        assert_eq!(
            routing.effective(Some("subscription")).unwrap().uuid,
            "assigned"
        );
        assert_eq!(routing.effective(None).unwrap().uuid, "global");
        routing.assignments.get_mut("subscription").unwrap().enabled = false;
        assert_eq!(
            routing.effective(Some("subscription")).unwrap().uuid,
            "global"
        );
        routing.default_profile_uuid = None;
        assert!(routing.effective(Some("subscription")).is_none());
        assert!(
            routing.is_assigned("assigned"),
            "disabled assignments still refer to this profile"
        );
    }
}
