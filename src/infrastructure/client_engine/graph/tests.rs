use super::*;
use crate::client_normalize::SubscriptionNormalizer;
use vpn_application::client::ClientNormalizerPort;
use vpn_domain::client::{GraphGroup, GraphOutbound, GraphRule, ProxyProfile};
use vpn_domain::routing::{RouteRule, RouteTarget, RoutingInput};

fn profile() -> ProxyProfile {
    let first = SubscriptionNormalizer
        .normalize(b"socks5://127.0.0.1:1081#A")
        .unwrap()
        .remove(0);
    let second = SubscriptionNormalizer
        .normalize(b"socks5://127.0.0.1:1082#B")
        .unwrap()
        .remove(0);
    let mut container = first.clone();
    container.protocol = ProxyProtocol::Graph(Box::new(ProxyGraph {
        outbounds: vec![
            GraphOutbound {
                tag: "direct".into(),
                kind: GraphOutboundKind::Proxy(Box::new(first)),
            },
            GraphOutbound {
                tag: "blazer-outbound".into(),
                kind: GraphOutboundKind::Proxy(Box::new(second)),
            },
            GraphOutbound {
                tag: "system-direct".into(),
                kind: GraphOutboundKind::Direct,
            },
            GraphOutbound {
                tag: "reject".into(),
                kind: GraphOutboundKind::Block,
            },
        ],
        groups: vec![GraphGroup {
            tag: "balanced".into(),
            members: vec!["direct".into(), "blazer-outbound".into()],
            strategy: GraphStrategy::RoundRobin,
        }],
        rules: vec![
            GraphRule {
                condition: GraphMatch::Domain(vec!["first.route.invalid".into()]),
                target: GraphTarget::Outbound("direct".into()),
            },
            GraphRule {
                condition: GraphMatch::DomainSuffix(vec!["route.invalid".into()]),
                target: GraphTarget::Outbound("blazer-outbound".into()),
            },
            GraphRule {
                condition: GraphMatch::Cidr(vec!["198.51.100.0/24".into()]),
                target: GraphTarget::Group("balanced".into()),
            },
            GraphRule {
                condition: GraphMatch::All(vec![
                    GraphMatch::Network(vec![GraphNetwork::Tcp]),
                    GraphMatch::Any(vec![
                        GraphMatch::Domain(vec!["nested.invalid".into()]),
                        GraphMatch::Cidr(vec!["2001:db8::/32".into()]),
                    ]),
                ]),
                target: GraphTarget::Outbound("system-direct".into()),
            },
            GraphRule {
                condition: GraphMatch::Domain(vec!["blocked.invalid".into()]),
                target: GraphTarget::Outbound("reject".into()),
            },
        ],
        default_target: GraphTarget::Group("balanced".into()),
        dns: DnsPolicy::Automatic,
        observatory: None,
    }));
    container
}

#[test]
fn least_ping_uses_a_selector_with_explicit_initial_fallback() {
    let mut profile = profile();
    let ProxyProtocol::Graph(provider) = &mut profile.protocol else {
        unreachable!()
    };
    provider.groups[0].strategy = GraphStrategy::LeastPing {
        fallback: Some("system-direct".into()),
    };
    provider.observatory = Some(vpn_domain::client::GraphObservatory {
        members: vec!["direct".into(), "blazer-outbound".into()],
        probe_url: vpn_domain::client::Secret::new("http://probe.fixture.invalid/"),
        interval: std::time::Duration::from_millis(200),
        enable_concurrency: true,
    });
    let records: Vec<Value> =
        serde_json::from_slice(&config::render(&profile, 19080, None).unwrap()).unwrap();
    assert_eq!(records[3]["type"], "selector");
    assert_eq!(records[3]["default"], "blazer-graph-outbound-2");
    assert_eq!(records[3]["client_chains"].as_array().unwrap().len(), 3);
    assert!(
        records
            .iter()
            .all(|record| record.get("clash_api_address").is_none())
    );
}

#[test]
fn full_graph_preserves_every_outbound_group_and_ordered_policy() {
    let records: Vec<Value> =
        serde_json::from_slice(&config::render(&profile(), 19080, None).unwrap()).unwrap();
    assert_eq!(records.len(), 5);
    assert_eq!(records[0]["client_chain"]["address"], "127.0.0.1:1081");
    assert_eq!(records[1]["client_chain"]["address"], "127.0.0.1:1082");
    assert_eq!(records[2]["client_chain"]["protocol"]["type"], "direct");
    assert_eq!(records[3]["type"], "round-robin");
    assert_eq!(
        records[3]["client_chains"],
        json!([
            {"chain": ["blazer-graph-outbound-0"]}, {"chain": ["blazer-graph-outbound-1"]}
        ])
    );
    let rules = records.last().unwrap()["rules"].as_array().unwrap();
    assert_provider_rules(rules, &records[3]["client_group"]);
}

fn assert_provider_rules(rules: &[Value], group: &Value) {
    assert_eq!(rules.len(), 6);
    assert_eq!(
        rules[0]["conditions"],
        json!({"domain": ["first.route.invalid"]})
    );
    assert_eq!(
        rules[1]["conditions"],
        json!({"domain_suffix": ["route.invalid"]})
    );
    assert_eq!(rules[2]["client_chain"], "blazer-graph-group-0");
    assert_eq!(
        rules[3]["conditions"]["all"][1]["any"][1],
        json!({"ip_cidr": ["2001:db8::/32"]})
    );
    assert_eq!(rules[4]["action"], "block");
    assert!(rules[4].get("client_chain").is_none());
    assert_eq!(rules[5]["conditions"], json!({"any_destination": true}));
    assert_eq!(&rules[5]["client_chain"], group);
    assert!(rules.iter().all(|rule| rule["no_resolve"] == true));
}

#[test]
fn user_proxy_rule_enters_provider_graph_before_later_user_rules() {
    let routing = RoutingProfile {
        uuid: "local-policy".into(),
        revision: 1,
        input: RoutingInput {
            title: "Local policy".into(),
            dns: DnsPolicy::System,
            rules: vec![
                RouteRule {
                    target: RouteTarget::DomainSuffix("route.invalid".into()),
                    action: RouteAction::Proxy,
                },
                RouteRule {
                    target: RouteTarget::Cidr("198.51.100.0/24".into()),
                    action: RouteAction::Block,
                },
                RouteRule {
                    target: RouteTarget::Domain("first.route.invalid".into()),
                    action: RouteAction::Direct,
                },
            ],
        },
    };
    let records: Vec<Value> =
        serde_json::from_slice(&config::render(&profile(), 19080, Some(&routing)).unwrap())
            .unwrap();
    let listener = records.last().unwrap();
    let rules = listener["rules"].as_array().unwrap();
    assert_eq!(rules.len(), 14);
    assert_eq!(
        rules[0]["conditions"],
        json!({"all": [{"domain_suffix": ["route.invalid"]}, {"domain": ["first.route.invalid"]}]})
    );
    assert_eq!(rules[0]["client_chain"], "blazer-graph-outbound-0");
    assert_eq!(
        rules[5]["conditions"],
        json!({"domain_suffix": ["route.invalid"]})
    );
    assert_eq!(rules[5]["client_chain"], "blazer-graph-group-0");
    assert_eq!(rules[6]["action"], "block");
    assert_eq!(rules[7]["client_chain"]["protocol"]["type"], "direct");
    assert_eq!(
        rules[8]["conditions"],
        json!({"domain": ["first.route.invalid"]})
    );
    assert_eq!(listener["dns"], json!({"servers": ["system"]}));
}

#[cfg(any(windows, target_os = "linux"))]
#[test]
fn managed_tunnel_dns_references_the_group_without_flattening_its_members() {
    let records = super::super::managed_tunnel::document(&profile(), None).unwrap();
    let tunnel = records.last().unwrap();
    assert_eq!(tunnel["auto_route"], true);
    assert_eq!(
        tunnel["dns"]["servers"][0]["client_chain"],
        records[3]["client_group"]
    );
    assert_eq!(tunnel["rules"].as_array().unwrap().len(), 6);
    assert!(tunnel.get("protocol").is_none());
}

#[test]
fn a_block_outbound_cannot_be_silently_used_as_a_proxy_group_member() {
    let mut profile = profile();
    let ProxyProtocol::Graph(graph) = &mut profile.protocol else {
        unreachable!()
    };
    graph.groups[0].members.push("reject".into());
    assert_eq!(
        config::validate(&profile),
        Err(ClientError::UnsupportedFeature)
    );
}

#[cfg(windows)]
#[test]
#[ignore = "requires the pinned Windows BlazeCore binary via BLAZER_TEST_BLAZECORE_EXE"]
fn native_blazecore_accepts_full_graph_for_proxy_and_managed_tun() {
    let executable =
        std::path::PathBuf::from(std::env::var_os("BLAZER_TEST_BLAZECORE_EXE").unwrap());
    assert_eq!(
        super::super::file_sha256(&executable).unwrap(),
        super::super::APPROVED_ENGINE_SHA256
    );
    let directory =
        std::env::temp_dir().join(format!("blazer-graph-render-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&directory).unwrap();
    let path = directory.join("profile.yaml");
    let documents = [
        config::render(&profile(), 19080, None).unwrap(),
        super::super::managed_tunnel::render(&profile(), None).unwrap(),
    ];
    for document in documents {
        std::fs::write(&path, document).unwrap();
        let checked = super::super::private::hidden_command(&executable)
            .arg("check")
            .arg(&path)
            .output()
            .unwrap();
        assert!(
            checked.status.success(),
            "native graph rejected: {}",
            String::from_utf8_lossy(&checked.stdout)
        );
    }
    std::fs::remove_file(path).unwrap();
    std::fs::remove_dir(directory).unwrap();
}
