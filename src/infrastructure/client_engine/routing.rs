use serde_json::{Value, json};
use vpn_domain::routing::{DnsPolicy, RouteAction, RouteRule, RouteTarget, RoutingInput};

pub(super) const PROXY_GROUP: &str = "blazer-outbound";

pub(super) fn proxy_group(proxy: Value) -> Value {
    json!({"client_group": PROXY_GROUP, "client_chains": [{"chain": [proxy]}]})
}

pub(super) fn configure_listener(listener: &mut Value, routing: &RoutingInput) {
    let mut rules: Vec<_> = routing.rules.iter().map(render_rule).collect();
    rules.push(json!({
        "conditions": {"any_destination": true}, "action": "allow", "client_chain": PROXY_GROUP
    }));
    listener["rules"] = json!(rules);
    if let Some(server) = dns_server(&routing.dns) {
        listener["dns"] = json!({"servers": [server]});
    }
}

pub(super) fn render_rule(rule: &RouteRule) -> Value {
    let target = match &rule.target {
        RouteTarget::Domain(name) => json!({"domain": [name]}),
        RouteTarget::DomainSuffix(name) => json!({"domain_suffix": [name]}),
        RouteTarget::Cidr(cidr) => json!({"ip_cidr": [cidr]}),
    };
    match rule.action {
        RouteAction::Proxy => json!({
            "conditions": target, "action": "allow", "client_chain": PROXY_GROUP
        }),
        RouteAction::Direct => json!({
            "conditions": target, "action": "allow",
            "client_chain": {"protocol": {"type": "direct"}}
        }),
        RouteAction::Block => json!({"conditions": target, "action": "block"}),
    }
}

pub(super) fn dns_server(policy: &DnsPolicy) -> Option<Value> {
    // Configure the listener's resolver. A remote proxy may resolve its own targets.
    let server = match policy {
        DnsPolicy::Automatic => return None,
        DnsPolicy::System => json!("system"),
        DnsPolicy::Tcp { endpoint } => json!(format!("tcp://{endpoint}")),
        DnsPolicy::Tls {
            endpoint,
            server_name,
        } => json!({"url": format!("tls://{endpoint}"), "server_name": server_name}),
        DnsPolicy::Https {
            endpoint,
            server_name,
            path,
        } => json!({"url": format!("https://{endpoint}{path}"), "server_name": server_name}),
    };
    Some(server)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routing_preserves_rule_order_and_an_unmatched_proxy_route() {
        let policy = RoutingInput {
            title: "Route fixture".into(),
            rules: vec![
                RouteRule {
                    target: RouteTarget::Domain("blocked.example".into()),
                    action: RouteAction::Block,
                },
                RouteRule {
                    target: RouteTarget::DomainSuffix("example".into()),
                    action: RouteAction::Direct,
                },
                RouteRule {
                    target: RouteTarget::Cidr("192.0.2.0/24".into()),
                    action: RouteAction::Proxy,
                },
                RouteRule {
                    target: RouteTarget::Cidr("::/0".into()),
                    action: RouteAction::Block,
                },
            ],
            dns: DnsPolicy::Automatic,
        };
        let mut listener = json!({});
        configure_listener(&mut listener, &policy);
        assert_eq!(
            listener,
            json!({"rules": [
                {"conditions": {"domain": ["blocked.example"]}, "action": "block"},
                {"conditions": {"domain_suffix": ["example"]}, "action": "allow",
                    "client_chain": {"protocol": {"type": "direct"}}},
                {"conditions": {"ip_cidr": ["192.0.2.0/24"]}, "action": "allow", "client_chain": PROXY_GROUP},
                {"conditions": {"ip_cidr": ["::/0"]}, "action": "block"},
                {"conditions": {"any_destination": true}, "action": "allow", "client_chain": PROXY_GROUP}
            ]})
        );
    }

    #[test]
    fn routing_dns_keeps_literal_endpoints_and_tls_names_separate() {
        let endpoint = "[2001:db8::53]:8853".parse().unwrap();
        assert_eq!(dns_server(&DnsPolicy::Automatic), None);
        assert_eq!(dns_server(&DnsPolicy::System), Some(json!("system")));
        assert_eq!(
            dns_server(&DnsPolicy::Tcp { endpoint }),
            Some(json!("tcp://[2001:db8::53]:8853"))
        );
        assert_eq!(
            dns_server(&DnsPolicy::Tls {
                endpoint,
                server_name: "dns.example".into()
            }),
            Some(json!({"url": "tls://[2001:db8::53]:8853", "server_name": "dns.example"}))
        );
        assert_eq!(
            dns_server(&DnsPolicy::Https {
                endpoint,
                server_name: "dns.example".into(),
                path: "/dns-query".into(),
            }),
            Some(
                json!({"url": "https://[2001:db8::53]:8853/dns-query", "server_name": "dns.example"})
            )
        );
    }
}
