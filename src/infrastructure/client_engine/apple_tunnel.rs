use serde::Serialize;
use serde_json::{Value, json};
use vpn_domain::client::{ClientError, ProxyProfile};
use vpn_domain::routing::RoutingProfile;
use zeroize::Zeroizing;

use super::{config, routing};

const MAX_REQUEST_BYTES: usize = 64 * 1024;
const MTU: u16 = 1500;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ProviderRequest<'a> {
    endpoint: &'a str,
    mtu: u16,
    records: Vec<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    observatory_profile: Option<&'a str>,
}

pub fn render_apple_tunnel(
    profile: &ProxyProfile,
    rules: Option<&RoutingProfile>,
) -> Result<Zeroizing<Vec<u8>>, ClientError> {
    if let Some(rules) = rules {
        rules.input.validate()?;
    }
    let chain = config::entry_chain(profile)?;
    let dns_chain = if matches!(
        profile.protocol,
        vpn_domain::client::ProxyProtocol::Graph(_)
    ) {
        chain.clone()
    } else {
        json!(routing::PROXY_GROUP)
    };
    let listener = json!({
        "device_name": "Blazer",
        "mtu": MTU,
        "dns_hijack": true,
        "dns": {"servers": [{"url": "tcp://1.1.1.1", "client_chain": dns_chain}]}
    });
    let records = if !matches!(
        profile.protocol,
        vpn_domain::client::ProxyProtocol::Graph(_)
    ) && rules.is_none()
    {
        let mut listener = listener;
        routing::configure_listener(&mut listener, &Default::default());
        vec![routing::proxy_group(chain), listener]
    } else {
        config::configure_profile_ingress(listener, profile, rules)?
    };
    let observatory_profile = if super::graph::needs_observer(profile) {
        Some(
            profile
                .source
                .as_ref()
                .ok_or(ClientError::InvalidInput)?
                .expose(),
        )
    } else {
        None
    };
    let request = ProviderRequest {
        endpoint: &profile.host,
        mtu: MTU,
        records,
        observatory_profile,
    };
    let bytes =
        Zeroizing::new(serde_json::to_vec(&request).map_err(|_| ClientError::EngineInvalid)?);
    if bytes.len() > MAX_REQUEST_BYTES {
        return Err(ClientError::PayloadTooLarge);
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client_normalize::SubscriptionNormalizer;
    use vpn_application::client::ClientNormalizerPort;
    use vpn_domain::routing::{DnsPolicy, RouteAction, RouteRule, RouteTarget, RoutingInput};

    #[test]
    fn emits_packet_flow_without_a_descriptor_or_duplicate_credentials() {
        let profile = SubscriptionNormalizer
            .normalize(b"trojan://fixture-secret@192.0.2.20:443#Fixture")
            .unwrap()
            .remove(0);
        let request = render_apple_tunnel(&profile, None).unwrap();
        let source = std::str::from_utf8(&request).unwrap();
        assert_eq!(source.matches("fixture-secret").count(), 1);

        let request: Value = serde_json::from_str(source).unwrap();
        assert_eq!(request["endpoint"], "192.0.2.20");
        let listener = &request["records"][1];
        assert_eq!(listener["mtu"], request["mtu"]);
        assert_eq!(listener["device_name"], "Blazer");
        assert!(listener.get("device_fd").is_none());
        assert!(listener.get("auto_route").is_none());
        assert_eq!(listener["dns_hijack"], true);
        assert_eq!(
            listener["dns"]["servers"][0]["client_chain"],
            "blazer-outbound"
        );
    }

    #[test]
    fn applies_ordered_rules_and_the_selected_dns_policy() {
        let profile = SubscriptionNormalizer
            .normalize(b"socks5://192.0.2.20:1080#Fixture")
            .unwrap()
            .remove(0);
        let rules = RoutingProfile {
            uuid: "fixture".into(),
            revision: 1,
            input: RoutingInput {
                title: "Fixture".into(),
                rules: vec![RouteRule {
                    target: RouteTarget::Cidr("203.0.113.0/24".into()),
                    action: RouteAction::Block,
                }],
                dns: DnsPolicy::Tcp {
                    endpoint: "192.0.2.53:53".parse().unwrap(),
                },
            },
        };
        let bytes = render_apple_tunnel(&profile, Some(&rules)).unwrap();
        let request: Value = serde_json::from_slice(&bytes).unwrap();
        let listener = &request["records"][1];
        assert_eq!(listener["rules"][0]["action"], "block");
        assert_eq!(listener["rules"][1]["client_chain"], "blazer-outbound");
        assert_eq!(listener["dns"]["servers"][0], "tcp://192.0.2.53:53");
    }

    #[test]
    fn apple_request_preserves_full_graph_and_embedded_observer_source() {
        let source = serde_json::json!({
            "outbounds": [
                {"tag":"fallback","protocol":"socks","settings":{"servers":[{"address":"127.0.0.1","port":1081}]}},
                {"tag":"candidate","protocol":"socks","settings":{"servers":[{"address":"127.0.0.1","port":1082}]}}
            ],
            "routing":{"domainStrategy":"AsIs","balancers":[{"tag":"fastest","selector":["candidate"],"strategy":{"type":"leastPing"},"fallbackTag":"fallback"}],
                "rules":[{"type":"field","network":"tcp","balancerTag":"fastest"}]},
            "observatory":{"subjectSelector":["candidate"],"probeURL":"http://probe.invalid/","probeInterval":"1s"}
        });
        let profile = SubscriptionNormalizer
            .normalize(source.to_string().as_bytes())
            .unwrap()
            .remove(0);
        let bytes = render_apple_tunnel(&profile, None).unwrap();
        let request: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(request["records"].as_array().unwrap().len(), 4);
        assert_eq!(request["records"][2]["type"], "selector");
        assert_eq!(request["records"][3]["device_name"], "Blazer");
        assert!(
            request["records"]
                .as_array()
                .unwrap()
                .iter()
                .all(|record| record.get("clash_api_address").is_none())
        );
        let restored: Value =
            serde_json::from_str(request["observatoryProfile"].as_str().unwrap()).unwrap();
        assert_eq!(restored, source);
    }
}
