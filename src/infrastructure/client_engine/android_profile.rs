//! Android adds an embedded selector capability gate to the strict shared validator.

use vpn_domain::client::{ClientError, GraphStrategy, ProxyProfile, ProxyProtocol};

pub fn validate_android_profile(
    profile: &ProxyProfile,
    selector_available: bool,
) -> Result<(), ClientError> {
    if let ProxyProtocol::Graph(graph) = &profile.protocol
        && !selector_available
        && graph
            .groups
            .iter()
            .any(|group| matches!(group.strategy, GraphStrategy::LeastPing { .. }))
    {
        return Err(ClientError::UnsupportedFeature);
    }
    super::validate_profile(profile)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use serde_json::{Value, json};
    use vpn_application::client::ClientNormalizerPort;
    use vpn_domain::client::{
        GraphGroup, GraphObservatory, GraphOutbound, GraphOutboundKind, GraphTarget, ProxyGraph,
        ProxySecurity, ProxyTransport, Secret,
    };

    use super::*;
    use crate::client_normalize::SubscriptionNormalizer;

    fn profile(input: Value) -> ProxyProfile {
        SubscriptionNormalizer
            .normalize(input.to_string().as_bytes())
            .unwrap()
            .remove(0)
    }

    fn least_ping(member: ProxyProfile) -> ProxyProfile {
        let mut profile = member.clone();
        profile.transport = ProxyTransport::Tcp;
        profile.security = ProxySecurity::None;
        profile.protocol = ProxyProtocol::Graph(Box::new(ProxyGraph {
            outbounds: vec![GraphOutbound {
                tag: "candidate".into(),
                kind: GraphOutboundKind::Proxy(Box::new(member)),
            }],
            groups: vec![GraphGroup {
                tag: "fastest".into(),
                members: vec!["candidate".into()],
                strategy: GraphStrategy::LeastPing { fallback: None },
            }],
            rules: vec![],
            default_target: GraphTarget::Outbound("candidate".into()),
            dns: Default::default(),
            observatory: Some(GraphObservatory {
                members: vec!["candidate".into()],
                probe_url: Secret::new("http://probe.invalid/"),
                interval: Duration::from_secs(1),
                enable_concurrency: true,
            }),
        }));
        profile
    }

    #[test]
    fn android_least_ping_keeps_advanced_xhttp_metadata_and_selector_gate() {
        let member = profile(json!({
            "type":"vless","server":"localhost","server_port":7443,
            "uuid":"fa9c41b7-fc31-4ab0-a275-1c67aa9aab31",
            "transport":{"type":"xhttp","mode":"packet-up","path":"/api",
                "session_placement":"cookie","seq_placement":"header","seq_key":"X-Packet",
                "sc_max_each_post_bytes":"128000-256000","sc_min_posts_interval_ms":5,
                "uplink_http_method":"GET"}
        }));
        assert!(
            matches!(&member.transport, ProxyTransport::Xhttp { packet, .. } if !packet.wire.is_empty())
        );
        let graph = least_ping(member);
        assert_eq!(
            validate_android_profile(&graph, false),
            Err(ClientError::UnsupportedFeature)
        );
        assert_eq!(validate_android_profile(&graph, true), Ok(()));
        let records =
            super::super::config::render_android_tunnel_records(&graph, 42, 1080).unwrap();
        let wire = &records[0]["client_chain"]["protocol"];
        assert_eq!(wire["session_id_placement"], "cookie");
        assert_eq!(wire["seq_key"], "X-Packet");
        assert_eq!(wire["sc_max_each_post_bytes"], "128000-256000");
        assert_eq!(wire["sc_min_posts_interval_ms"], 5);
        assert_eq!(wire["uplink_http_method"], "GET");
        assert_eq!(records[1]["type"], "selector");
        assert_eq!(records[2]["device_fd"], 42);
    }

    #[test]
    fn android_native_vless_and_wireguard_use_the_strict_shared_chain() {
        let protocols = [
            json!({"type":"vless","user_id":"fa9c41b7-fc31-4ab0-a275-1c67aa9aab31"}),
            json!({"type":"wireguard",
                "private_key":"KioqKioqKioqKioqKioqKioqKioqKioqKioqKioqKio=",
                "public_key":"F9firNML9eGdhhroCYjRf66PZi9dgLAFP2zHPNqx0mQ=",
                "addresses":["10.77.0.2/32"],"allowed_ips":["0.0.0.0/0"]}),
        ];
        for protocol in protocols {
            let mut member =
                profile(json!({"type":"socks","server":"127.0.0.1","server_port":7443}));
            let (native, transport) =
                crate::native_profile::parse(json!({"protocol":protocol}), vec![]).unwrap();
            member.protocol = ProxyProtocol::Native(native);
            member.transport = transport;
            assert!(matches!(member.protocol, ProxyProtocol::Native(_)));
            assert_eq!(validate_android_profile(&member, false), Ok(()));
            let graph = least_ping(member);
            assert_eq!(validate_android_profile(&graph, true), Ok(()));
            let records =
                super::super::config::render_android_tunnel_records(&graph, 42, 1080).unwrap();
            assert_eq!(records[0]["client_chain"]["protocol"], protocol);
        }
    }
}
