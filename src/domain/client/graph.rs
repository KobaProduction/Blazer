//! A bounded, nonrecursive graph of proxy endpoints, groups and routing rules.

mod selection;
pub use selection::{ProbeReport, record_graph_probe, select_least_ping};

use std::collections::HashSet;
use std::fmt;
use std::net::IpAddr;
use std::time::Duration;

use super::{ClientError, MAX_PROFILES, ProxyProfile, ProxyProtocol, TcpProbeEndpoint};
use crate::routing::DnsPolicy;

#[derive(Clone, PartialEq, Eq)]
pub struct ProxyGraph {
    pub outbounds: Vec<GraphOutbound>,
    pub groups: Vec<GraphGroup>,
    pub rules: Vec<GraphRule>,
    pub default_target: GraphTarget,
    pub dns: DnsPolicy,
    pub observatory: Option<GraphObservatory>,
}

#[derive(Clone, PartialEq, Eq)]
pub struct GraphObservatory {
    pub members: Vec<String>,
    pub probe_url: super::Secret,
    pub interval: Duration,
    pub enable_concurrency: bool,
}

#[derive(Clone, PartialEq, Eq)]
pub struct GraphOutbound {
    pub tag: String,
    pub kind: GraphOutboundKind,
}

#[derive(Clone, PartialEq, Eq)]
pub enum GraphOutboundKind {
    Proxy(Box<ProxyProfile>),
    Direct,
    Block,
}

#[derive(Clone, PartialEq, Eq)]
pub struct GraphGroup {
    pub tag: String,
    pub members: Vec<String>,
    pub strategy: GraphStrategy,
}

#[derive(Clone, PartialEq, Eq)]
pub enum GraphStrategy {
    RoundRobin,
    LeastPing { fallback: Option<String> },
}

#[derive(Clone, PartialEq, Eq)]
pub enum GraphTarget {
    Outbound(String),
    Group(String),
}

#[derive(Clone, PartialEq, Eq)]
pub struct GraphRule {
    pub condition: GraphMatch,
    pub target: GraphTarget,
}

#[derive(Clone, PartialEq, Eq)]
pub enum GraphMatch {
    AnyDestination,
    Domain(Vec<String>),
    DomainSuffix(Vec<String>),
    Cidr(Vec<String>),
    Network(Vec<GraphNetwork>),
    Any(Vec<GraphMatch>),
    All(Vec<GraphMatch>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphNetwork {
    Tcp,
    Udp,
}

impl fmt::Debug for ProxyGraph {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProxyGraph")
            .field("outbounds", &self.outbounds.len())
            .field("groups", &self.groups.len())
            .field("rules", &self.rules.len())
            .finish_non_exhaustive()
    }
}

macro_rules! redacted_debug {
    ($($kind:ty),+ $(,)?) => {
        $(impl fmt::Debug for $kind {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.debug_struct(stringify!($kind)).finish_non_exhaustive()
            }
        })+
    };
}

redacted_debug!(
    GraphOutbound,
    GraphOutboundKind,
    GraphGroup,
    GraphTarget,
    GraphRule,
    GraphMatch,
    GraphObservatory,
    GraphStrategy
);

impl ProxyGraph {
    pub fn validate(&self) -> Result<(), ClientError> {
        if self.outbounds.is_empty() {
            return Err(ClientError::InvalidInput);
        }
        if self.outbounds.len() > MAX_PROFILES
            || self.groups.len() > MAX_PROFILES
            || self.rules.len() > MAX_PROFILES
        {
            return Err(ClientError::PayloadTooLarge);
        }
        let outbound_tags = unique_tags(self.outbounds.iter().map(|item| item.tag.as_str()))?;
        let group_tags = unique_tags(self.groups.iter().map(|item| item.tag.as_str()))?;
        if !outbound_tags.is_disjoint(&group_tags) {
            return Err(ClientError::InvalidInput);
        }
        self.validate_outbounds()?;
        self.validate_observatory(&outbound_tags)?;
        for group in &self.groups {
            validate_group(group, &outbound_tags)?;
            self.validate_strategy(group, &outbound_tags)?;
        }
        validate_target(&self.default_target, &outbound_tags, &group_tags)?;
        let mut remaining = MAX_PROFILES;
        for rule in &self.rules {
            validate_target(&rule.target, &outbound_tags, &group_tags)?;
            validate_match(&rule.condition, 1, &mut remaining)?;
        }
        self.dns.validate()
    }

    fn validate_outbounds(&self) -> Result<(), ClientError> {
        for outbound in &self.outbounds {
            if let GraphOutboundKind::Proxy(profile) = &outbound.kind {
                if matches!(profile.protocol, ProxyProtocol::Graph(_)) {
                    return Err(ClientError::UnsupportedFeature);
                }
                if TcpProbeEndpoint::new(&profile.host, profile.port).is_none() {
                    return Err(ClientError::InvalidInput);
                }
            }
        }
        Ok(())
    }

    fn validate_observatory(&self, outbounds: &HashSet<&str>) -> Result<(), ClientError> {
        let Some(observer) = &self.observatory else {
            return Ok(());
        };
        if observer.members.len() > MAX_PROFILES {
            return Err(ClientError::PayloadTooLarge);
        }
        let members = unique_tags(observer.members.iter().map(String::as_str))?;
        if !members.is_subset(outbounds)
            || observer.interval.is_zero()
            || observer.interval > Duration::from_secs(86_400)
        {
            return Err(ClientError::InvalidInput);
        }
        // Reuse the domain's HTTP URL contract; the observer's request budget is
        // fixed separately and is not the interactive ping preference.
        super::ProbeSettings {
            url: observer.probe_url.expose().to_owned(),
            ..Default::default()
        }
        .validate()
    }

    fn validate_strategy(
        &self,
        group: &GraphGroup,
        outbounds: &HashSet<&str>,
    ) -> Result<(), ClientError> {
        if let GraphStrategy::LeastPing { fallback } = &group.strategy {
            if self.observatory.is_none() {
                return Err(ClientError::UnsupportedFeature);
            }
            if fallback.is_none() && matches!(self.default_target, GraphTarget::Group(_)) {
                return Err(ClientError::UnsupportedFeature);
            }
            if fallback
                .as_deref()
                .is_some_and(|tag| !outbounds.contains(tag))
            {
                return Err(ClientError::InvalidInput);
            }
        }
        Ok(())
    }
}

fn unique_tags<'a>(tags: impl Iterator<Item = &'a str>) -> Result<HashSet<&'a str>, ClientError> {
    let mut result = HashSet::new();
    for tag in tags {
        if tag.is_empty()
            || tag.trim() != tag
            || tag.len() > 127
            || tag.chars().any(char::is_control)
            || !result.insert(tag)
        {
            return Err(ClientError::InvalidInput);
        }
    }
    Ok(result)
}

fn validate_group(group: &GraphGroup, outbounds: &HashSet<&str>) -> Result<(), ClientError> {
    if group.members.is_empty() {
        return Err(ClientError::InvalidInput);
    }
    if group.members.len() > MAX_PROFILES {
        return Err(ClientError::PayloadTooLarge);
    }
    let members = unique_tags(group.members.iter().map(String::as_str))?;
    if !members.is_subset(outbounds) {
        return Err(ClientError::InvalidInput);
    }
    Ok(())
}

fn validate_target(
    target: &GraphTarget,
    outbounds: &HashSet<&str>,
    groups: &HashSet<&str>,
) -> Result<(), ClientError> {
    let valid = match target {
        GraphTarget::Outbound(tag) => outbounds.contains(tag.as_str()),
        GraphTarget::Group(tag) => groups.contains(tag.as_str()),
    };
    valid.then_some(()).ok_or(ClientError::InvalidInput)
}

fn validate_match(value: &GraphMatch, depth: u8, remaining: &mut usize) -> Result<(), ClientError> {
    if depth > 8 || *remaining == 0 {
        return Err(ClientError::PayloadTooLarge);
    }
    *remaining -= 1;
    match value {
        GraphMatch::AnyDestination => Ok(()),
        GraphMatch::Domain(values) | GraphMatch::DomainSuffix(values) => {
            validate_values(values, super::valid_dns_probe_host)
        }
        GraphMatch::Cidr(values) => validate_values(values, valid_cidr),
        GraphMatch::Network(values) => {
            let unique = values.len() <= 2 && (values.len() < 2 || values[0] != values[1]);
            (unique && !values.is_empty())
                .then_some(())
                .ok_or(ClientError::InvalidInput)
        }
        GraphMatch::Any(values) | GraphMatch::All(values) => {
            if values.is_empty() {
                return Err(ClientError::InvalidInput);
            }
            for value in values {
                validate_match(value, depth + 1, remaining)?;
            }
            Ok(())
        }
    }
}

fn validate_values(values: &[String], valid: impl Fn(&str) -> bool) -> Result<(), ClientError> {
    if values.len() > MAX_PROFILES {
        return Err(ClientError::PayloadTooLarge);
    }
    (!values.is_empty() && values.iter().all(|value| valid(value)))
        .then_some(())
        .ok_or(ClientError::InvalidInput)
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
    use crate::client::{ProxySecurity, ProxyTransport, Secret};

    fn fixture() -> ProxyGraph {
        ProxyGraph {
            outbounds: vec![
                GraphOutbound {
                    tag: "first".into(),
                    kind: GraphOutboundKind::Direct,
                },
                GraphOutbound {
                    tag: "second".into(),
                    kind: GraphOutboundKind::Block,
                },
            ],
            groups: vec![GraphGroup {
                tag: "balanced".into(),
                members: vec!["first".into(), "second".into()],
                strategy: GraphStrategy::RoundRobin,
            }],
            rules: vec![GraphRule {
                condition: GraphMatch::All(vec![
                    GraphMatch::Any(vec![
                        GraphMatch::Domain(vec!["one.invalid".into()]),
                        GraphMatch::DomainSuffix(vec!["two.invalid".into()]),
                    ]),
                    GraphMatch::Cidr(vec!["192.0.2.0/24".into(), "2001:db8::/32".into()]),
                    GraphMatch::Network(vec![GraphNetwork::Tcp, GraphNetwork::Udp]),
                ]),
                target: GraphTarget::Outbound("first".into()),
            }],
            default_target: GraphTarget::Group("balanced".into()),
            dns: DnsPolicy::System,
            observatory: None,
        }
    }

    #[test]
    fn graph_requires_unambiguous_outbound_group_and_default_references() {
        let original = fixture();
        original.validate().unwrap();
        let mut graph = original.clone();
        graph.groups[0].members[0] = "missing".into();
        assert_eq!(graph.validate(), Err(ClientError::InvalidInput));
        graph = original.clone();
        graph.groups[0].members.clear();
        assert_eq!(graph.validate(), Err(ClientError::InvalidInput));
        graph = original.clone();
        graph.groups[0].members[0] = "balanced".into();
        assert_eq!(graph.validate(), Err(ClientError::InvalidInput));
        graph = original.clone();
        graph.groups[0].tag = "first".into();
        assert_eq!(graph.validate(), Err(ClientError::InvalidInput));
        graph = original.clone();
        graph.default_target = GraphTarget::Outbound("balanced".into());
        assert_eq!(graph.validate(), Err(ClientError::InvalidInput));
    }

    #[test]
    fn least_ping_requires_a_valid_shared_observer_and_existing_fallback() {
        let mut graph = fixture();
        graph.groups[0].strategy = GraphStrategy::LeastPing {
            fallback: Some("second".into()),
        };
        assert_eq!(graph.validate(), Err(ClientError::UnsupportedFeature));
        graph.observatory = Some(GraphObservatory {
            members: vec!["first".into()],
            probe_url: super::super::Secret::new(
                "https://probe.invalid/check?private-token=fixture",
            ),
            interval: Duration::from_millis(200),
            enable_concurrency: false,
        });
        graph.validate().unwrap();
        let original = graph.clone();
        graph.groups[0].strategy = GraphStrategy::LeastPing {
            fallback: Some("missing".into()),
        };
        assert_eq!(graph.validate(), Err(ClientError::InvalidInput));
        graph = original.clone();
        graph
            .observatory
            .as_mut()
            .unwrap()
            .members
            .push("missing".into());
        assert_eq!(graph.validate(), Err(ClientError::InvalidInput));
        graph = original;
        graph.observatory.as_mut().unwrap().members.clear();
        graph.validate().unwrap();
        assert!(!format!("{:?}", graph.observatory).contains("private-token"));
        graph.groups[0].strategy = GraphStrategy::LeastPing { fallback: None };
        assert_eq!(graph.validate(), Err(ClientError::UnsupportedFeature));
        graph.default_target = GraphTarget::Outbound("first".into());
        graph.validate().unwrap();
    }

    #[test]
    fn observer_rejects_unbounded_intervals_and_non_http_targets() {
        let mut graph = fixture();
        let observer = GraphObservatory {
            members: vec![],
            probe_url: super::super::Secret::new("https://probe.invalid/check"),
            interval: Duration::from_secs(10),
            enable_concurrency: true,
        };
        graph.observatory = Some(observer.clone());
        for interval in [Duration::ZERO, Duration::from_secs(86_401)] {
            graph.observatory.as_mut().unwrap().interval = interval;
            assert_eq!(graph.validate(), Err(ClientError::InvalidInput));
        }
        graph.observatory = Some(observer);
        graph.observatory.as_mut().unwrap().probe_url =
            super::super::Secret::new("file:///private");
        assert_eq!(graph.validate(), Err(ClientError::InvalidUrl));
    }

    #[test]
    fn graph_rejects_deep_or_oversized_match_trees_and_noncanonical_cidrs() {
        let mut graph = fixture();
        let mut condition = GraphMatch::AnyDestination;
        for _ in 0..8 {
            condition = GraphMatch::Any(vec![condition]);
        }
        graph.rules[0].condition = condition;
        assert_eq!(graph.validate(), Err(ClientError::PayloadTooLarge));
        graph.rules[0].condition = GraphMatch::Any(vec![GraphMatch::AnyDestination; MAX_PROFILES]);
        assert_eq!(graph.validate(), Err(ClientError::PayloadTooLarge));
        graph.rules[0].condition = GraphMatch::Cidr(vec!["192.0.2.1/24".into()]);
        assert_eq!(graph.validate(), Err(ClientError::InvalidInput));
        graph.rules[0].condition = GraphMatch::Network(Vec::new());
        assert_eq!(graph.validate(), Err(ClientError::InvalidInput));
    }

    #[test]
    fn graph_debug_redacts_tags_profiles_addresses_and_credentials_and_rejects_nested_graphs() {
        let mut graph = fixture();
        let endpoint = ProxyProfile {
            profile_uuid: "private-uuid".into(),
            name: "private-name".into(),
            source: Some(Secret::new("private-source")),
            host: "private-host.invalid".into(),
            port: 443,
            protocol: ProxyProtocol::Socks5 {
                username: Some(Secret::new("private-user")),
                password: Some(Secret::new("private-password")),
            },
            transport: ProxyTransport::Tcp,
            security: ProxySecurity::None,
        };
        graph.outbounds[0].tag = "private-tag".into();
        graph.outbounds[0].kind = GraphOutboundKind::Proxy(Box::new(endpoint.clone()));
        let debug = format!(
            "{graph:?} {:?} {:?} {:?} {:?}",
            graph.outbounds[0], graph.groups[0], graph.rules[0], graph.default_target
        );
        assert!(!debug.contains("private-"));
        let mut nested = endpoint;
        nested.protocol = ProxyProtocol::Graph(Box::new(fixture()));
        graph = fixture();
        graph.outbounds[0].kind = GraphOutboundKind::Proxy(Box::new(nested));
        assert_eq!(graph.validate(), Err(ClientError::UnsupportedFeature));
    }
}
