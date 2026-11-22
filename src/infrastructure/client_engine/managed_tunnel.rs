use serde_json::{Value, json};
use vpn_domain::client::{ClientError, ProxyProfile};
use vpn_domain::routing::RoutingProfile;

use super::config;

pub(super) fn render(
    profile: &ProxyProfile,
    routing: Option<&RoutingProfile>,
) -> Result<Vec<u8>, ClientError> {
    serde_json::to_vec(&document(profile, routing)?).map_err(|_| ClientError::EngineInvalid)
}

pub(super) fn document(
    profile: &ProxyProfile,
    routing: Option<&RoutingProfile>,
) -> Result<Vec<Value>, ClientError> {
    let chain = config::entry_chain(profile)?;
    let mut tunnel = json!({
        "auto_route": true,
        "addresses": ["10.233.0.1/30", "fd73:686f:6573::1/126"],
        "mtu": 1500,
        "bootstrap_dns": ["1.1.1.1:53", "9.9.9.9:53"],
        "auto_dns": true,
        "dns_hijack": true,
        "dns": {"servers": [{"url": "tcp://1.1.1.1", "client_chain": &chain}]}
    });
    if let Some(dns) = config::profile_dns(profile, &chain) {
        tunnel["dns"] = dns;
    }
    if cfg!(target_os = "linux") {
        tunnel["device_name"] = json!("blazer0");
    }
    config::configure_profile_ingress(tunnel, profile, routing)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client_normalize::SubscriptionNormalizer;
    use vpn_application::client::ClientNormalizerPort;
    use vpn_domain::routing::{DnsPolicy, RouteAction, RouteRule, RouteTarget, RoutingInput};

    #[test]
    fn managed_tun_keeps_the_selected_outbound_and_routing_policy() {
        let profile = SubscriptionNormalizer
            .normalize(b"socks5://127.0.0.1:1080#Fixture")
            .unwrap()
            .remove(0);
        let routing = RoutingProfile {
            uuid: "test".into(),
            revision: 1,
            input: RoutingInput {
                title: "TUN routing".into(),
                rules: vec![RouteRule {
                    target: RouteTarget::Cidr("198.51.100.0/24".into()),
                    action: RouteAction::Block,
                }],
                dns: DnsPolicy::Automatic,
            },
        };
        let records = document(&profile, Some(&routing)).unwrap();
        let tun = &records[1];
        assert_eq!(tun["auto_route"], true);
        assert_eq!(tun["auto_dns"], true);
        assert_eq!(tun["rules"][0]["action"], "block");
        assert_eq!(tun["rules"][1]["client_chain"], records[0]["client_group"]);
        assert_eq!(
            records[0]["client_chains"][0]["chain"][0]["address"],
            "127.0.0.1:1080"
        );
        assert!(tun.get("protocol").is_none());
        if cfg!(target_os = "linux") {
            assert_eq!(tun["device_name"], "blazer0");
        } else {
            assert!(tun.get("device_name").is_none());
        }
    }
}
