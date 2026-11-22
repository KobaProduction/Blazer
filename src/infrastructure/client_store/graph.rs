//! Canonical graph storage. This DTO is covered by the catalog's encryption.

use serde::{Deserialize, Serialize};
use vpn_domain::client::{
    CatalogProfile, ClientError, GraphGroup, GraphMatch, GraphNetwork, GraphObservatory,
    GraphOutbound, GraphOutboundKind, GraphRule, GraphStrategy, GraphTarget, MAX_PROFILES,
    ProxyGraph,
};

use super::{DnsPolicyDto, ProtocolDto, StoredProfile};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct GraphDto {
    outbounds: Vec<OutboundDto>,
    groups: Vec<GroupDto>,
    rules: Vec<RuleDto>,
    default_target: TargetDto,
    dns: DnsPolicyDto,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    observatory: Option<ObservatoryDto>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ObservatoryDto {
    members: Vec<String>,
    probe_url: String,
    interval_nanos: u64,
    enable_concurrency: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OutboundDto {
    tag: String,
    kind: OutboundKindDto,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum OutboundKindDto {
    Proxy { profile: Box<StoredProfile> },
    Direct,
    Block,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct GroupDto {
    tag: String,
    members: Vec<String>,
    strategy: StrategyDto,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
enum StrategyDto {
    RoundRobin,
    LeastPing {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fallback: Option<String>,
    },
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum TargetDto {
    Outbound { tag: String },
    Group { tag: String },
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RuleDto {
    condition: MatchDto,
    target: TargetDto,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum MatchDto {
    AnyDestination,
    Domain { values: Vec<String> },
    DomainSuffix { values: Vec<String> },
    Cidr { values: Vec<String> },
    Network { values: Vec<NetworkDto> },
    Any { conditions: Vec<MatchDto> },
    All { conditions: Vec<MatchDto> },
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum NetworkDto {
    Tcp,
    Udp,
}

impl GraphDto {
    pub(super) fn from_graph(graph: &ProxyGraph) -> Self {
        Self {
            outbounds: graph.outbounds.iter().map(OutboundDto::from).collect(),
            groups: graph.groups.iter().map(GroupDto::from).collect(),
            rules: graph.rules.iter().map(RuleDto::from).collect(),
            default_target: TargetDto::from(&graph.default_target),
            dns: DnsPolicyDto::from(&graph.dns),
            observatory: graph.observatory.as_ref().map(ObservatoryDto::from),
        }
    }

    pub(super) fn into_graph(self) -> Result<ProxyGraph, ClientError> {
        if self.outbounds.len() > MAX_PROFILES
            || self.groups.len() > MAX_PROFILES
            || self.rules.len() > MAX_PROFILES
        {
            return Err(ClientError::StorageCorrupt);
        }
        let mut remaining = MAX_PROFILES;
        let graph = ProxyGraph {
            outbounds: self
                .outbounds
                .into_iter()
                .map(OutboundDto::into_outbound)
                .collect::<Result<_, _>>()?,
            groups: self.groups.into_iter().map(GraphGroup::from).collect(),
            rules: self
                .rules
                .into_iter()
                .map(|rule| {
                    Ok(GraphRule {
                        condition: rule.condition.into_match(1, &mut remaining)?,
                        target: GraphTarget::from(rule.target),
                    })
                })
                .collect::<Result<_, ClientError>>()?,
            default_target: GraphTarget::from(self.default_target),
            dns: self.dns.into(),
            observatory: self.observatory.map(GraphObservatory::from),
        };
        graph.validate().map_err(|_| ClientError::StorageCorrupt)?;
        Ok(graph)
    }
}

impl From<&GraphOutbound> for OutboundDto {
    fn from(outbound: &GraphOutbound) -> Self {
        let kind = match &outbound.kind {
            GraphOutboundKind::Proxy(profile) => OutboundKindDto::Proxy {
                profile: Box::new(StoredProfile::from_profile(&CatalogProfile {
                    profile: (**profile).clone(),
                    subscription_uuid: None,
                })),
            },
            GraphOutboundKind::Direct => OutboundKindDto::Direct,
            GraphOutboundKind::Block => OutboundKindDto::Block,
        };
        Self {
            tag: outbound.tag.clone(),
            kind,
        }
    }
}

impl OutboundDto {
    fn into_outbound(self) -> Result<GraphOutbound, ClientError> {
        let kind = match self.kind {
            OutboundKindDto::Proxy { profile } => {
                // Reject nested graphs before recursively restoring a member profile.
                if matches!(profile.protocol, ProtocolDto::Graph { .. })
                    || profile.subscription_uuid.is_some()
                {
                    return Err(ClientError::StorageCorrupt);
                }
                GraphOutboundKind::Proxy(Box::new(profile.into_profile()?.profile))
            }
            OutboundKindDto::Direct => GraphOutboundKind::Direct,
            OutboundKindDto::Block => GraphOutboundKind::Block,
        };
        Ok(GraphOutbound {
            tag: self.tag,
            kind,
        })
    }
}

impl From<&GraphGroup> for GroupDto {
    fn from(group: &GraphGroup) -> Self {
        Self {
            tag: group.tag.clone(),
            members: group.members.clone(),
            strategy: match &group.strategy {
                GraphStrategy::RoundRobin => StrategyDto::RoundRobin,
                GraphStrategy::LeastPing { fallback } => StrategyDto::LeastPing {
                    fallback: fallback.clone(),
                },
            },
        }
    }
}

impl From<GroupDto> for GraphGroup {
    fn from(group: GroupDto) -> Self {
        Self {
            tag: group.tag,
            members: group.members,
            strategy: match group.strategy {
                StrategyDto::RoundRobin => GraphStrategy::RoundRobin,
                StrategyDto::LeastPing { fallback } => GraphStrategy::LeastPing { fallback },
            },
        }
    }
}

impl From<&GraphObservatory> for ObservatoryDto {
    fn from(observer: &GraphObservatory) -> Self {
        Self {
            members: observer.members.clone(),
            probe_url: observer.probe_url.expose().to_owned(),
            interval_nanos: u64::try_from(observer.interval.as_nanos()).unwrap_or(u64::MAX),
            enable_concurrency: observer.enable_concurrency,
        }
    }
}

impl From<ObservatoryDto> for GraphObservatory {
    fn from(observer: ObservatoryDto) -> Self {
        Self {
            members: observer.members,
            probe_url: vpn_domain::client::Secret::new(observer.probe_url),
            interval: std::time::Duration::from_nanos(observer.interval_nanos),
            enable_concurrency: observer.enable_concurrency,
        }
    }
}

impl From<&GraphTarget> for TargetDto {
    fn from(target: &GraphTarget) -> Self {
        match target {
            GraphTarget::Outbound(tag) => Self::Outbound { tag: tag.clone() },
            GraphTarget::Group(tag) => Self::Group { tag: tag.clone() },
        }
    }
}

impl From<TargetDto> for GraphTarget {
    fn from(target: TargetDto) -> Self {
        match target {
            TargetDto::Outbound { tag } => Self::Outbound(tag),
            TargetDto::Group { tag } => Self::Group(tag),
        }
    }
}

impl From<&GraphRule> for RuleDto {
    fn from(rule: &GraphRule) -> Self {
        Self {
            condition: MatchDto::from(&rule.condition),
            target: TargetDto::from(&rule.target),
        }
    }
}

impl From<&GraphMatch> for MatchDto {
    fn from(condition: &GraphMatch) -> Self {
        match condition {
            GraphMatch::AnyDestination => Self::AnyDestination,
            GraphMatch::Domain(values) => Self::Domain {
                values: values.clone(),
            },
            GraphMatch::DomainSuffix(values) => Self::DomainSuffix {
                values: values.clone(),
            },
            GraphMatch::Cidr(values) => Self::Cidr {
                values: values.clone(),
            },
            GraphMatch::Network(values) => Self::Network {
                values: values
                    .iter()
                    .map(|network| match network {
                        GraphNetwork::Tcp => NetworkDto::Tcp,
                        GraphNetwork::Udp => NetworkDto::Udp,
                    })
                    .collect(),
            },
            GraphMatch::Any(conditions) => Self::Any {
                conditions: conditions.iter().map(Self::from).collect(),
            },
            GraphMatch::All(conditions) => Self::All {
                conditions: conditions.iter().map(Self::from).collect(),
            },
        }
    }
}

impl MatchDto {
    fn into_match(self, depth: u8, remaining: &mut usize) -> Result<GraphMatch, ClientError> {
        if depth > 8 || *remaining == 0 {
            return Err(ClientError::StorageCorrupt);
        }
        *remaining -= 1;
        Ok(match self {
            Self::AnyDestination => GraphMatch::AnyDestination,
            Self::Domain { values } => GraphMatch::Domain(values),
            Self::DomainSuffix { values } => GraphMatch::DomainSuffix(values),
            Self::Cidr { values } => GraphMatch::Cidr(values),
            Self::Network { values } => GraphMatch::Network(
                values
                    .into_iter()
                    .map(|network| match network {
                        NetworkDto::Tcp => GraphNetwork::Tcp,
                        NetworkDto::Udp => GraphNetwork::Udp,
                    })
                    .collect(),
            ),
            Self::Any { conditions } => {
                GraphMatch::Any(restore_conditions(conditions, depth, remaining)?)
            }
            Self::All { conditions } => {
                GraphMatch::All(restore_conditions(conditions, depth, remaining)?)
            }
        })
    }
}

fn restore_conditions(
    conditions: Vec<MatchDto>,
    depth: u8,
    remaining: &mut usize,
) -> Result<Vec<GraphMatch>, ClientError> {
    conditions
        .into_iter()
        .map(|condition| condition.into_match(depth + 1, remaining))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(windows)]
    use vpn_domain::client::Catalog;
    use vpn_domain::client::{ProxyProfile, ProxyProtocol, ProxySecurity, ProxyTransport, Secret};
    use vpn_domain::routing::DnsPolicy;

    fn fixture() -> ProxyGraph {
        let proxy = |port| {
            GraphOutboundKind::Proxy(Box::new(ProxyProfile {
                profile_uuid: format!("endpoint-{port}"),
                name: format!("private-name-{port}"),
                source: Some(Secret::new("private-fixture-source")),
                host: "private-host.invalid".into(),
                port,
                protocol: ProxyProtocol::Socks5 {
                    username: Some(Secret::new("private-user")),
                    password: Some(Secret::new("private-password")),
                },
                transport: ProxyTransport::Tcp,
                security: ProxySecurity::None,
            }))
        };
        ProxyGraph {
            outbounds: vec![
                GraphOutbound {
                    tag: "first".into(),
                    kind: proxy(1080),
                },
                GraphOutbound {
                    tag: "second".into(),
                    kind: proxy(1081),
                },
                GraphOutbound {
                    tag: "direct".into(),
                    kind: GraphOutboundKind::Direct,
                },
                GraphOutbound {
                    tag: "block".into(),
                    kind: GraphOutboundKind::Block,
                },
            ],
            groups: vec![GraphGroup {
                tag: "balanced".into(),
                members: vec!["first".into(), "second".into()],
                strategy: GraphStrategy::RoundRobin,
            }],
            rules: vec![
                GraphRule {
                    condition: GraphMatch::All(vec![
                        GraphMatch::Any(vec![
                            GraphMatch::Domain(vec!["one.invalid".into()]),
                            GraphMatch::DomainSuffix(vec!["two.invalid".into()]),
                        ]),
                        GraphMatch::Network(vec![GraphNetwork::Tcp, GraphNetwork::Udp]),
                        GraphMatch::Cidr(vec!["192.0.2.0/24".into()]),
                    ]),
                    target: GraphTarget::Outbound("direct".into()),
                },
                GraphRule {
                    condition: GraphMatch::AnyDestination,
                    target: GraphTarget::Outbound("block".into()),
                },
            ],
            default_target: GraphTarget::Group("balanced".into()),
            dns: DnsPolicy::Https {
                endpoint: "1.1.1.1:443".parse().unwrap(),
                server_name: "dns.invalid".into(),
                path: "/dns-query".into(),
            },
            observatory: None,
        }
    }

    #[test]
    fn full_graph_storage_roundtrip_preserves_members_groups_rules_and_dns() {
        let graph = fixture();
        graph.validate().unwrap();
        let encoded = serde_json::to_vec(&GraphDto::from_graph(&graph)).unwrap();
        let restored = serde_json::from_slice::<GraphDto>(&encoded)
            .unwrap()
            .into_graph()
            .unwrap();
        assert_eq!(restored, graph);
        assert!(!format!("{restored:?}").contains("private-"));
    }

    fn least_ping_fixture() -> ProxyGraph {
        let mut graph = fixture();
        graph.groups[0].strategy = GraphStrategy::LeastPing {
            fallback: Some("direct".into()),
        };
        graph.observatory = Some(GraphObservatory {
            members: vec!["first".into()],
            probe_url: Secret::new("https://probe.invalid/check?private-url-token=fixture"),
            interval: std::time::Duration::from_millis(250),
            enable_concurrency: false,
        });
        graph
    }

    #[test]
    fn least_ping_storage_preserves_strategy_shared_observer_and_nanosecond_precision() {
        let mut graph = least_ping_fixture();
        graph.observatory.as_mut().unwrap().interval = std::time::Duration::from_nanos(250_000_001);
        graph.validate().unwrap();
        let encoded = serde_json::to_vec(&GraphDto::from_graph(&graph)).unwrap();
        let restored = serde_json::from_slice::<GraphDto>(&encoded)
            .unwrap()
            .into_graph()
            .unwrap();
        assert_eq!(restored, graph);
        assert!(!format!("{restored:?}").contains("private-url-token"));
    }

    #[test]
    fn least_ping_storage_rejects_corrupt_observation_and_fallback_references() {
        let baseline = serde_json::to_value(GraphDto::from_graph(&least_ping_fixture())).unwrap();
        for (field, value) in [
            ("interval_nanos", serde_json::json!(0)),
            ("members", serde_json::json!(["missing"])),
            ("probe_url", serde_json::json!("file:///private")),
        ] {
            let mut encoded = baseline.clone();
            encoded["observatory"][field] = value;
            let stored: GraphDto = serde_json::from_value(encoded).unwrap();
            assert_eq!(stored.into_graph(), Err(ClientError::StorageCorrupt));
        }
        let mut encoded = baseline;
        encoded["groups"][0]["strategy"]["least_ping"]["fallback"] = serde_json::json!("missing");
        let stored: GraphDto = serde_json::from_value(encoded).unwrap();
        assert_eq!(stored.into_graph(), Err(ClientError::StorageCorrupt));
    }

    #[test]
    fn least_ping_storage_rejects_unknown_strategy_options() {
        let mut encoded =
            serde_json::to_value(GraphDto::from_graph(&least_ping_fixture())).unwrap();
        encoded["groups"][0]["strategy"]["least_ping"]["unmapped"] = serde_json::json!(true);
        assert!(serde_json::from_value::<GraphDto>(encoded).is_err());
    }

    #[test]
    fn graph_storage_rejects_invalid_references_and_nested_graph_members() {
        let mut encoded = serde_json::to_value(GraphDto::from_graph(&fixture())).unwrap();
        encoded["groups"][0]["members"] = serde_json::json!(["missing"]);
        let stored: GraphDto = serde_json::from_value(encoded).unwrap();
        assert_eq!(stored.into_graph(), Err(ClientError::StorageCorrupt));
        let mut encoded = serde_json::to_value(GraphDto::from_graph(&fixture())).unwrap();
        let nested = serde_json::to_value(GraphDto::from_graph(&fixture())).unwrap();
        encoded["outbounds"][0]["kind"]["profile"]["protocol"] =
            serde_json::json!({"type":"graph", "graph":nested});
        let stored: GraphDto = serde_json::from_value(encoded).unwrap();
        assert_eq!(stored.into_graph(), Err(ClientError::StorageCorrupt));
    }

    #[test]
    fn graph_storage_rejects_deep_match_trees_before_domain_conversion() {
        let mut encoded = serde_json::to_value(GraphDto::from_graph(&fixture())).unwrap();
        let mut condition = serde_json::json!({"type":"any_destination"});
        for _ in 0..8 {
            condition = serde_json::json!({"type":"any", "conditions":[condition]});
        }
        encoded["rules"][0]["condition"] = condition;
        let stored: GraphDto = serde_json::from_value(encoded).unwrap();
        assert_eq!(stored.into_graph(), Err(ClientError::StorageCorrupt));
    }

    #[cfg(windows)]
    #[test]
    fn windows_dpapi_catalog_preserves_graph_without_plaintext_credentials() {
        use super::super::{SealedCatalogStore, StateStorePort};
        struct Directory(std::path::PathBuf);
        impl Drop for Directory {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let directory = Directory(
            std::env::temp_dir().join(format!("blazer-graph-store-{}", uuid::Uuid::new_v4())),
        );
        let store = SealedCatalogStore::new(directory.0.clone()).unwrap();
        let graph = least_ping_fixture();
        let catalog = Catalog {
            profiles: vec![CatalogProfile {
                subscription_uuid: None,
                profile: ProxyProfile {
                    profile_uuid: "graph".into(),
                    name: "Graph fixture".into(),
                    source: None,
                    host: "graph.invalid".into(),
                    port: 443,
                    protocol: ProxyProtocol::Graph(Box::new(graph)),
                    transport: ProxyTransport::Tcp,
                    security: ProxySecurity::None,
                },
            }],
            ..Catalog::default()
        };
        store.save(&catalog).unwrap();
        let ciphertext = std::fs::read(directory.0.join("catalog.sealed")).unwrap();
        assert!(
            !ciphertext
                .windows(b"private-password".len())
                .any(|window| window == b"private-password")
        );
        let restored = store.load().unwrap().unwrap();
        assert_eq!(restored.profiles, catalog.profiles);
    }
}
