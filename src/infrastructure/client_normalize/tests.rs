use super::*;
use vpn_domain::client::{
    XhttpMetadata, XhttpMode, XhttpPacketOptions, XhttpPacketRange, XhttpPlacement,
};

const ID: &str = "a5c41b77-fc31-4ab0-a275-1c67aa9aab31";

fn normalize(input: impl AsRef<[u8]>) -> Result<Vec<ProxyProfile>, ClientError> {
    let mut profiles = SubscriptionNormalizer.normalize(input.as_ref())?;
    // The dialect comparisons below assert equivalent connection settings.
    for profile in &mut profiles {
        profile.source = None;
    }
    Ok(profiles)
}

#[test]
fn wireguard_conf_preserves_amneziawg_3_1_fields_and_rejects_scripts() {
    let client_key = STANDARD.encode([42; 32]);
    let server_key = STANDARD.encode([41; 32]);
    let input = format!(
        "# Note\n[Interface]\nPrivateKey = {client_key}\nAddress = 10.8.0.2/32, fd00::2/128\nDNS = 10.8.0.1\nMTU = 1420\n\
         Jc = 8\nJmin = 75\nJmax = 123\nS1 = 40\nS2 = 97\nS3 = 20\nS4 = 16\n\
         H1 = 1000000-1000999\nH2 = 2000000-2000999\nH3 = 3000000-3000999\nH4 = 4000000-4000999\n\
         I1 = <b 0x160301><r 32>\nI2 = <r 28>\nHeaderProtectionKey = {client_key}\n\
         ContentPaddingAddition = 0-64\nRekeyAfterTime = 121-155\nRekeyTimeout = 5\nRejectAfterTime = 185-201\n\
         KeepaliveTimeout = 12-26\nMaxHandshakeAttempts = 18\nRandomTrailers = on\nDisableCookies = off\n\
         [Peer]\nPublicKey = {server_key}\nEndpoint = [2001:db8::1]:51820\n\
         AllowedIPs = 0.0.0.0/0, ::/0\nPersistentKeepalive = 20-30\n"
    );
    let profile = normalize(&input).unwrap().remove(0);
    let ProxyProtocol::Native(native) = &profile.protocol else {
        panic!("expected native AmneziaWG");
    };
    let protocol: serde_json::Value = serde_json::from_str(native.settings.expose()).unwrap();
    let awg = &protocol["protocol"]["amnezia"];
    assert_eq!(profile.protocol.name(), "AmneziaWG");
    assert_eq!(profile.host, "2001:db8::1");
    assert_eq!(profile.transport, ProxyTransport::Udp);
    assert_eq!(native.dns_servers[0].to_string(), "10.8.0.1");
    assert_eq!(awg["header_protection_key"], "2a".repeat(32));
    assert_eq!(awg["content_padding_addition"], "0-64");
    assert_eq!(awg["rekey_after_time"], "121-155");
    assert_eq!(awg["persistent_keepalive_interval"], "20-30");
    assert_eq!(awg["random_trailers"], true);
    assert_eq!(awg["disable_cookies"], false);
    assert_eq!(
        normalize(input.replace("MTU = 1420", "PreUp = echo reject")),
        Err(ClientError::UnsupportedFeature)
    );
    assert_eq!(
        normalize(input.replace("MTU = 1420", "MTU = 1800")),
        Err(ClientError::InvalidSubscription)
    );
}

#[test]
fn native_yaml_imports_client_group_and_rejects_listener_policies() {
    let yaml = r#"
- client_group: vpn
  client_chains:
    - address: "127.0.0.1:443"
      protocol:
        type: hysteria2
        password: secret
        hopping: {ports: "443, 8000-8003", interval_secs: 15}
- address: "127.0.0.1:1080"
  protocol: {type: socks}
  rules:
    - masks: ["0.0.0.0/0", "::/0"]
      action: allow
      client_chain: vpn
"#;
    let profile = normalize(yaml).unwrap().remove(0);
    assert_eq!(profile.protocol.name(), "Hysteria2");
    assert!(!profile.transport.has_tcp_endpoint());
    assert_eq!(
        normalize(yaml.replace("action: allow", "action: block")),
        Err(ClientError::UnsupportedFeature)
    );
}

#[test]
fn amnezia_vpn_imports_qt_compressed_bundle_and_decoded_wireguard() {
    use std::io::Write as _;

    let private = STANDARD.encode([42; 32]);
    let public = STANDARD.encode([41; 32]);
    let config = format!(
        "[Interface]\nPrivateKey = {private}\nAddress = 10.8.0.2/32\n[Peer]\nPublicKey = {public}\nEndpoint = 198.51.100.2:51820\nAllowedIPs = 0.0.0.0/0"
    );
    let settings = serde_json::json!({
        "config": config.replace("[Peer]", "DNS = $PRIMARY_DNS, 192.0.2.53, $SECONDARY_DNS\nI2 =\nI3 = \nI4 =\nI5 =\n[Peer]"),
        "Jc":"2", "Jmin":"12", "Jmax":"20",
        "HeaderProtectionKey":private, "S1":12, "S2":"12", "S3":12, "S4":12,
        "RandomTrailers":"on", "I1":"<r 5>", "I2":"", "I3":"  ", "I4":"", "I5":""
    });
    let bundle = serde_json::json!({
        "description":"Amnezia fixture", "defaultContainer":"amnezia-awg2",
        "dns1":"10.8.0.1", "dns2":"2001:db8::53",
        "containers":[
            {"container":"amnezia-openvpn"},
            {"container":"amnezia-awg2","awg":{"last_config":settings.to_string()}}
        ]
    })
    .to_string();
    let mut encoder = flate2::write::ZlibEncoder::new(
        (bundle.len() as u32).to_be_bytes().to_vec(),
        flate2::Compression::default(),
    );
    encoder.write_all(bundle.as_bytes()).unwrap();
    let compressed = encoder.finish().unwrap();
    let link = |value: &[u8]| format!("vpn://{}", URL_SAFE_NO_PAD.encode(value));
    check_amnezia_editor_roundtrip(&link(&compressed));
    for input in [link(&compressed), link(bundle.as_bytes()), bundle] {
        let profile = normalize(input).unwrap().remove(0);
        assert_eq!(profile.name, "Amnezia fixture");
        let ProxyProtocol::Native(proxy) = profile.protocol else {
            panic!("expected native AmneziaWG");
        };
        assert_eq!(proxy.kind.name(), "AmneziaWG");
        let settings: serde_json::Value = serde_json::from_str(proxy.settings.expose()).unwrap();
        assert_eq!(settings["protocol"]["amnezia"]["random_trailers"], true);
        let servers = proxy
            .dns_servers
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        assert_eq!(servers, ["10.8.0.1", "192.0.2.53", "2001:db8::53"]);
    }
    assert_eq!(
        normalize(link(config.as_bytes())).unwrap()[0]
            .protocol
            .name(),
        "WireGuard"
    );
    assert_eq!(
        normalize(link(&compressed[..compressed.len() - 2])),
        Err(ClientError::InvalidSubscription)
    );
    assert_eq!(
        normalize(link(&u32::MAX.to_be_bytes())),
        Err(ClientError::PayloadTooLarge)
    );
}

fn check_amnezia_editor_roundtrip(input: &str) {
    let imported = SubscriptionNormalizer
        .normalize(input.as_bytes())
        .unwrap()
        .remove(0);
    let mut editor: serde_json::Value =
        serde_json::from_str(imported.source.as_ref().unwrap().expose()).unwrap();
    assert_eq!(editor["protocol"]["type"], "amneziawg");
    assert_eq!(editor["protocol"]["amnezia"]["i1"], "<r 5>");
    for unused in ["i2", "i3", "i4", "i5"] {
        assert!(editor["protocol"]["amnezia"].get(unused).is_none());
    }
    assert_eq!(editor["protocol"]["amnezia"]["random_trailers"], true);
    editor["protocol"]["amnezia"]["random_trailers"] = serde_json::json!(false);
    editor["dns_servers"] = serde_json::json!(["192.0.2.53"]);
    let edited = normalize(editor.to_string()).unwrap().remove(0);
    let ProxyProtocol::Native(edited) = edited.protocol else {
        panic!("the edited profile keeps AmneziaWG");
    };
    assert_eq!(edited.dns_servers[0].to_string(), "192.0.2.53");
    let updated: serde_json::Value = serde_json::from_str(edited.settings.expose()).unwrap();
    assert_eq!(updated["protocol"]["type"], "wireguard");
    assert_eq!(updated["protocol"]["amnezia"]["random_trailers"], false);
}

#[test]
fn amnezia_vpn_imports_structured_wireguard_without_native_conf() {
    let private = STANDARD.encode([42; 32]);
    let public = STANDARD.encode([41; 32]);
    let document = serde_json::json!({
        "hostName":"2001:db8::1","defaultContainer":"amnezia-wireguard",
        "containers":[{"container":"amnezia-wireguard","wireguard":{
            "port":"51820","last_config":{
                "client_ip":"10.8.0.2","client_priv_key":private,"server_pub_key":public,
                "allowed_ips":["0.0.0.0/0"]
            }
        }}]
    });
    let source = format!("vpn://{}", URL_SAFE.encode(document.to_string()));
    let profile = normalize(source).unwrap().remove(0);
    assert_eq!(profile.host, "2001:db8::1");
    assert_eq!(profile.port, 51820);
    assert_eq!(profile.protocol.name(), "WireGuard");
}

#[test]
fn fetched_base64_and_plain_links_keep_usable_profiles_and_safe_labels() {
    use vpn_application::client::{ProfileUnavailableReason, UnavailableProfile};

    let source = format!(
        "socks5://localhost:1080#First\nvless://{ID}@example.test:443?type=xhttp&extra=present#Second%20profile\n\
         vless://fixture-secret@example.test:443#https%3A%2F%2Fexample.test%2Ftoken\nsocks5://localhost:1081#Fourth"
    );
    for input in [source.clone(), URL_SAFE_NO_PAD.encode(&source)] {
        let result = SubscriptionNormalizer
            .normalize_subscription(input.as_bytes())
            .unwrap();
        assert_eq!(
            result
                .profiles
                .iter()
                .map(|profile| profile.name.as_str())
                .collect::<Vec<_>>(),
            ["First", "Fourth"]
        );
        assert_eq!(
            result.unavailable_profiles,
            [
                UnavailableProfile::new(
                    "Second profile",
                    Some("vless"),
                    ProfileUnavailableReason::Unsupported
                )
                .with_probe_endpoint(vpn_domain::client::TcpProbeEndpoint::new(
                    "example.test",
                    443
                )),
                UnavailableProfile::new("", Some("vless"), ProfileUnavailableReason::Invalid),
            ]
        );
        assert!(normalize(input).is_err());
    }
}

#[test]
fn fetched_xray_configurations_preserve_remarks_without_dropping_dns_or_routing() {
    use vpn_application::client::ProfileUnavailableReason;

    let names = ["One", "Two", "Three", "Four", "Five", "Six"];
    let documents = names.map(|name| serde_json::json!({
        "remarks": name,
        "dns": {"servers": ["example.test"]},
        "routing": {"rules": [{"outboundTag": "proxy"}]},
        "outbounds": [
            {
                "protocol": "vless", "tag": "proxy",
                "settings": {"vnext": [{"address": "example.test", "port": 443, "users": [{"id": ID}]}]},
                "streamSettings": {
                    "network": "xhttp", "xhttpSettings": {"path": "/", "extra": {"scMaxBufferedPosts": 12}},
                    "security": "tls", "tlsSettings": {"fingerprint": "chrome"}
                }
            },
            {"protocol": "freedom", "tag": "direct"},
            {"protocol": "blackhole", "tag": "blocked"}
        ]
    }));
    let body = serde_json::to_vec(&documents).unwrap();
    let result = SubscriptionNormalizer
        .normalize_subscription(&body)
        .unwrap();
    assert!(result.profiles.is_empty());
    assert_eq!(result.unavailable_profiles.len(), names.len());
    for (entry, name) in result.unavailable_profiles.iter().zip(names) {
        assert_eq!(entry.name, name);
        assert_eq!(entry.protocol.as_deref(), Some("vless"));
        assert_eq!(entry.reason, ProfileUnavailableReason::Unsupported);
    }
    assert_eq!(normalize(body), Err(ClientError::UnsupportedFeature));
}

#[test]
fn fetched_wrappers_isolate_independent_outbounds_and_keep_single_profile_remarks() {
    use vpn_application::client::ProfileUnavailableReason;

    let input = br#"[
        {"remarks":"Readable name","outbounds":[{"protocol":"http","tag":"proxy","settings":{"address":"localhost","port":80}},{"protocol":"freedom"}]},
        {"outbounds":[{"protocol":"socks","tag":"Second","settings":{"address":"localhost","port":1080}},
            {"protocol":"vmess","tag":"Broken","settings":{}},{"protocol":"blackhole"}]},
        null
    ]"#;
    let result = SubscriptionNormalizer
        .normalize_subscription(input)
        .unwrap();
    assert_eq!(
        result
            .profiles
            .iter()
            .map(|profile| profile.name.as_str())
            .collect::<Vec<_>>(),
        ["Readable name", "Second"]
    );
    assert_eq!(result.unavailable_profiles.len(), 2);
    assert_eq!(result.unavailable_profiles[0].name, "Broken");
    assert_eq!(
        result.unavailable_profiles[0].reason,
        ProfileUnavailableReason::Invalid
    );
    assert_eq!(result.unavailable_profiles[1].protocol, None);
}

fn balanced_xray_configuration() -> serde_json::Value {
    // Synthetic endpoints using Project X's BalancerObject/ObservatoryObject:
    // https://xtls.github.io/config/routing.html#balancerobject
    // https://xtls.github.io/config/observatory.html#observatoryobject
    serde_json::json!({
        "remarks": "Balanced route",
        "log": {"loglevel": "warning"},
        "inbounds": [{
            "tag": "local", "listen": "127.0.0.1", "port": 1080,
            "protocol": "socks", "settings": {"auth": "noauth", "udp": true}
        }],
        "outbounds": [
            {"tag": "proxy-a", "protocol": "socks", "settings": {
                "servers": [{"address": "192.0.2.10", "port": 1081}]
            }},
            {"tag": "proxy-b", "protocol": "socks", "settings": {
                "servers": [{"address": "192.0.2.20", "port": 1082}]
            }}
        ],
        "routing": {
            "domainStrategy": "AsIs",
            "rules": [{"type": "field", "inboundTag": ["local"], "balancerTag": "smart"}],
            "balancers": [{
                "tag": "smart", "selector": ["proxy-"], "fallbackTag": "proxy-a",
                "strategy": {"type": "leastPing"}
            }]
        },
        "observatory": {
            "subjectSelector": ["proxy-"],
            "probeUrl": "https://connectivitycheck.gstatic.com/generate_204",
            "probeInterval": "10s", "enableConcurrency": true
        }
    })
}

#[test]
fn xray_least_ping_policy_is_not_flattened_and_preserves_the_complete_source() {
    let document = balanced_xray_configuration();
    let mut independent = document.clone();
    independent.as_object_mut().unwrap().remove("routing");
    independent.as_object_mut().unwrap().remove("observatory");
    independent.as_object_mut().unwrap().remove("remarks");
    let supported = normalize(independent.to_string()).unwrap();
    assert_eq!(supported.len(), 2);
    assert_eq!(supported[0].name, "proxy-a");
    assert_eq!(supported[1].name, "proxy-b");
    let body = serde_json::to_vec(&document).unwrap();
    for source in [body.clone(), STANDARD.encode(&body).into_bytes()] {
        let imported = SubscriptionNormalizer
            .normalize_subscription(&source)
            .unwrap();
        let manual = SubscriptionNormalizer.normalize(&source).unwrap();
        assert!(imported.unavailable_profiles.is_empty());
        assert_eq!(imported.profiles.len(), 1);
        assert_eq!(manual.len(), 1);
        assert_least_ping_profile(&imported.profiles[0], &document);
        assert_least_ping_profile(&manual[0], &document);
    }
}

fn assert_least_ping_profile(profile: &ProxyProfile, document: &serde_json::Value) {
    use vpn_domain::client::GraphStrategy;
    let ProxyProtocol::Graph(graph) = &profile.protocol else {
        panic!("the policy must remain one composite graph");
    };
    assert_eq!(profile.name, "Balanced route");
    assert_eq!(graph.groups[0].members, ["proxy-a", "proxy-b"]);
    assert_eq!(
        graph.groups[0].strategy,
        GraphStrategy::LeastPing {
            fallback: Some("proxy-a".into())
        }
    );
    let observer = graph.observatory.as_ref().unwrap();
    assert_eq!(observer.members, ["proxy-a", "proxy-b"]);
    assert_eq!(
        observer.probe_url.expose(),
        "https://connectivitycheck.gstatic.com/generate_204"
    );
    assert_eq!(observer.interval, std::time::Duration::from_secs(10));
    assert!(observer.enable_concurrency);
    let source: serde_json::Value =
        serde_json::from_str(profile.source.as_ref().unwrap().expose()).unwrap();
    assert_eq!(&source, document);
}

#[test]
fn xray_least_ping_policy_keeps_independent_subscription_entries() {
    let document = serde_json::json!([
        balanced_xray_configuration(),
        "socks5://192.0.2.30:1083#Independent%20server"
    ]);
    let imported = SubscriptionNormalizer
        .normalize_subscription(document.to_string().as_bytes())
        .unwrap();
    let [policy, independent] = imported.profiles.as_slice() else {
        panic!("the graph and the separate share link must both remain selectable");
    };
    assert!(matches!(policy.protocol, ProxyProtocol::Graph(_)));
    assert_eq!(policy.name, "Balanced route");
    assert_eq!(independent.name, "Independent server");
    assert_eq!(independent.host, "192.0.2.30");
    assert_eq!(independent.port, 1083);
    assert!(imported.unavailable_profiles.is_empty());
}

fn graph_from_document(document: &serde_json::Value) -> Box<vpn_domain::client::ProxyGraph> {
    let profile = SubscriptionNormalizer
        .normalize(document.to_string().as_bytes())
        .unwrap()
        .remove(0);
    let ProxyProtocol::Graph(graph) = profile.protocol else {
        panic!("expected a graph");
    };
    graph
}

#[test]
fn xray_least_ping_observatory_keeps_its_own_subjects_defaults_and_precise_interval() {
    let mut source = balanced_xray_configuration();
    source["observatory"] = serde_json::json!({"subjectSelector":["proxy-b"]});
    let graph = graph_from_document(&source);
    let observer = graph.observatory.unwrap();
    assert_eq!(observer.members, ["proxy-b"]);
    assert_eq!(
        observer.probe_url.expose(),
        "https://www.google.com/generate_204"
    );
    assert_eq!(observer.interval, std::time::Duration::from_secs(10));
    assert!(!observer.enable_concurrency);
    for (value, expected) in [
        ("200ms", 200_000_000),
        (".5s", 500_000_000),
        ("1m30.5s", 90_500_000_000),
        ("1\u{b5}s", 1_000),
        ("0s", 10_000_000_000),
    ] {
        source["observatory"]["probeInterval"] = serde_json::json!(value);
        assert_eq!(
            graph_from_document(&source).observatory.unwrap().interval,
            std::time::Duration::from_nanos(expected)
        );
    }
    source["observatory"]["probeURL"] = serde_json::json!("https://probe.example/check");
    assert_eq!(
        graph_from_document(&source)
            .observatory
            .unwrap()
            .probe_url
            .expose(),
        "https://probe.example/check"
    );
}

#[test]
fn xray_least_ping_rejects_conflicting_urls_missing_fallback_and_unrepresented_observer_policy() {
    let original = balanced_xray_configuration();
    let mut conflict = original.clone();
    conflict["observatory"]["probeURL"] = serde_json::json!("https://other.example/check");
    let mut missing_fallback = original.clone();
    missing_fallback["routing"]["balancers"][0]["fallbackTag"] = serde_json::json!("missing");
    let mut invalid_interval = original.clone();
    invalid_interval["observatory"]["probeInterval"] = serde_json::json!("-1s");
    for source in [conflict, missing_fallback, invalid_interval] {
        assert_eq!(
            normalize(source.to_string()),
            Err(ClientError::InvalidSubscription)
        );
    }
    let mut absent = original.clone();
    absent.as_object_mut().unwrap().remove("observatory");
    let mut unsupported = original.clone();
    unsupported["observatory"]["unmappedField"] = serde_json::json!(true);
    let mut long_interval = original.clone();
    long_interval["observatory"]["probeInterval"] = serde_json::json!("25h");
    for source in [absent, unsupported, long_interval] {
        assert_eq!(
            normalize(source.to_string()),
            Err(ClientError::UnsupportedFeature)
        );
    }
}

fn round_robin_xray_configuration() -> serde_json::Value {
    let mut document = balanced_xray_configuration();
    document.as_object_mut().unwrap().remove("observatory");
    let balancer = &mut document["routing"]["balancers"][0];
    balancer.as_object_mut().unwrap().remove("fallbackTag");
    balancer["strategy"]["type"] = serde_json::json!("roundRobin");
    document
}

#[test]
fn xray_round_robin_graph_preserves_members_rules_default_and_complete_editor_source() {
    let mut document = round_robin_xray_configuration();
    document["outbounds"].as_array_mut().unwrap().extend([
        serde_json::json!({"tag":"direct", "protocol":"freedom", "settings":{}}),
        serde_json::json!({"tag":"blocked", "protocol":"blackhole", "settings":{}}),
    ]);
    document["routing"]["balancers"][0]["selector"] = serde_json::json!(["proxy-", "proxy-a"]);
    document["routing"]["rules"] = serde_json::json!([
        {"type":"field", "domain":["full:ads.example"], "outboundTag":"blocked"},
        {"type":"field", "ip":["192.0.2.25/24", "2001:db8::12"], "outboundTag":"direct"},
        {"type":"field", "domain":["full:exact.example", "domain:example.net"],
            "network":"tcp,udp", "inboundTag":["local"], "balancerTag":"smart"}
    ]);
    let body = serde_json::to_vec(&document).unwrap();
    for input in [body.clone(), STANDARD.encode(&body).into_bytes()] {
        let manual = SubscriptionNormalizer.normalize(&input).unwrap();
        let fetched = SubscriptionNormalizer
            .normalize_subscription(&input)
            .unwrap();
        assert!(fetched.unavailable_profiles.is_empty());
        assert_eq!(manual.len(), 1);
        assert_eq!(fetched.profiles.len(), 1);
        for profile in manual.iter().chain(&fetched.profiles) {
            assert_composite_xray_profile(profile, &document);
        }
    }
}

fn assert_composite_xray_profile(profile: &ProxyProfile, document: &serde_json::Value) {
    assert_eq!(profile.name, "Balanced route");
    assert_eq!((profile.host.as_str(), profile.port), ("192.0.2.10", 1081));
    let source: serde_json::Value =
        serde_json::from_str(profile.source.as_ref().unwrap().expose()).unwrap();
    assert_eq!(&source, document);
    let ProxyProtocol::Graph(graph) = &profile.protocol else {
        panic!("the entire Xray document must become one composite profile");
    };
    assert_round_robin_graph(graph);
}

fn assert_round_robin_graph(graph: &vpn_domain::client::ProxyGraph) {
    use vpn_domain::client::{
        GraphMatch, GraphNetwork, GraphOutboundKind, GraphStrategy, GraphTarget,
    };

    assert_eq!(
        graph
            .outbounds
            .iter()
            .map(|outbound| outbound.tag.as_str())
            .collect::<Vec<_>>(),
        ["proxy-a", "proxy-b", "direct", "blocked"]
    );
    assert!(matches!(graph.outbounds[2].kind, GraphOutboundKind::Direct));
    assert!(matches!(graph.outbounds[3].kind, GraphOutboundKind::Block));
    assert_eq!(
        graph.default_target,
        GraphTarget::Outbound("proxy-a".into())
    );
    assert_eq!(graph.groups[0].members, ["proxy-a", "proxy-b"]);
    assert_eq!(graph.groups[0].strategy, GraphStrategy::RoundRobin);
    assert_eq!(
        graph.rules[0].target,
        GraphTarget::Outbound("blocked".into())
    );
    assert_eq!(
        graph.rules[1].condition,
        GraphMatch::Cidr(vec!["192.0.2.0/24".into(), "2001:db8::12/128".into()])
    );
    assert_eq!(
        graph.rules[2].condition,
        GraphMatch::All(vec![
            GraphMatch::Any(vec![
                GraphMatch::Domain(vec!["exact.example".into()]),
                GraphMatch::DomainSuffix(vec!["example.net".into()])
            ]),
            GraphMatch::Network(vec![GraphNetwork::Tcp, GraphNetwork::Udp]),
        ])
    );
    assert_eq!(graph.rules[2].target, GraphTarget::Group("smart".into()));
}

#[test]
fn xray_round_robin_graph_rejects_missing_references_and_unmapped_policy_semantics() {
    let source = round_robin_xray_configuration();
    let mut missing_group = source.clone();
    missing_group["routing"]["rules"][0]["balancerTag"] = serde_json::json!("missing");
    let mut missing_outbound = source.clone();
    missing_outbound["routing"]["rules"][0]
        .as_object_mut()
        .unwrap()
        .remove("balancerTag");
    missing_outbound["routing"]["rules"][0]["outboundTag"] = serde_json::json!("missing");
    let mut empty_selector = source.clone();
    empty_selector["routing"]["balancers"][0]["selector"] = serde_json::json!(["missing-"]);
    for document in [missing_group, missing_outbound, empty_selector] {
        assert_eq!(
            normalize(document.to_string()),
            Err(ClientError::InvalidSubscription)
        );
    }

    let mut unknown_strategy = source.clone();
    unknown_strategy["routing"]["balancers"][0]["strategy"]["type"] = serde_json::json!("random");
    let mut default_random = source.clone();
    default_random["routing"]["balancers"][0]
        .as_object_mut()
        .unwrap()
        .remove("strategy");
    let mut fallback = source.clone();
    fallback["routing"]["balancers"][0]["fallbackTag"] = serde_json::json!("proxy-a");
    let mut conflicting_inbounds = source.clone();
    let mut inbound = conflicting_inbounds["inbounds"][0].clone();
    inbound["tag"] = serde_json::json!("second");
    conflicting_inbounds["inbounds"]
        .as_array_mut()
        .unwrap()
        .push(inbound);
    let mut custom_dns = source.clone();
    custom_dns["dns"] = serde_json::json!({"servers":["1.1.1.1"]});
    let mut geo_rule = source.clone();
    geo_rule["routing"]["rules"][0]["domain"] = serde_json::json!(["geosite:private"]);
    let mut authenticated_inbound = source.clone();
    authenticated_inbound["inbounds"][0]["settings"] =
        serde_json::json!({"auth":"password", "accounts":[{"user":"fixture", "pass":"fixture"}]});
    for document in [
        unknown_strategy,
        default_random,
        fallback,
        conflicting_inbounds,
        custom_dns,
        geo_rule,
        authenticated_inbound,
    ] {
        assert_eq!(
            normalize(document.to_string()),
            Err(ClientError::UnsupportedFeature)
        );
    }
}

#[test]
fn fetched_subscription_rejects_malformed_documents_and_checks_combined_limit() {
    let malformed = [
        &b"<html><a href=\"socks5://localhost:1080\">Expired</a></html>"[..],
        br#"{"error":"expired","outbounds":[{"protocol":"socks","settings":{"address":"localhost","port":1080}}]}"#,
        br#"[{"type":"error","description":"expired"}]"#,
        br#"[{"protocol":"socks","settings":{"address":"localhost","port":1080}},{"type":"vless","type":"socks"}]"#,
        b"\xff",
    ];
    for input in malformed {
        assert!(matches!(
            SubscriptionNormalizer.normalize_subscription(input),
            Err(ClientError::InvalidSubscription)
        ));
    }
    let oversized = "socks5://localhost:1080#First\n".to_owned()
        + &"tuic://fixture-secret@example.test:443#Other\n".repeat(MAX_PROFILES);
    assert!(matches!(
        SubscriptionNormalizer.normalize_subscription(oversized.as_bytes()),
        Err(ClientError::PayloadTooLarge)
    ));
}

#[test]
fn plain_and_four_base64_variants_share_one_bounded_dispatcher() {
    let text = "\u{feff}http://fixture-user:fixture-pass@127.0.0.1:8273#HTTP\r\n\r\nsocks5://[::1]:1080#SOCKS\r\n";
    let expected = normalize(text).expect("plain URI bundle");
    assert_eq!(expected.len(), 2);
    assert_eq!(expected[1].host, "::1");
    for engine in [&STANDARD, &STANDARD_NO_PAD, &URL_SAFE, &URL_SAFE_NO_PAD] {
        let encoded = engine.encode(text);
        let wrapped = format!("\u{feff}{}\r\n \t{}", &encoded[..20], &encoded[20..]);
        assert_eq!(normalize(wrapped), Ok(expected.clone()));
    }
    let double_encoded = STANDARD.encode(STANDARD.encode(text));
    assert!(normalize(double_encoded).is_err());
    let mut max = vec![b'\n'; MAX_SUBSCRIPTION_BYTES];
    max[..text.len()].copy_from_slice(text.as_bytes());
    assert_eq!(normalize(&max), Ok(expected));
    max.push(b'\n');
    assert_eq!(normalize(max), Err(ClientError::PayloadTooLarge));
}

#[test]
fn xray_modern_legacy_and_base64_json_have_equal_profiles() {
    let modern = format!(
        r#"{{"protocol":"vmess","tag":"Fixture","settings":{{"address":"localhost","port":7443,"id":"{ID}","security":"aes-128-gcm","level":0,"experiments":""}},"streamSettings":{{"method":"websocket","wsSettings":{{"path":"/vmess","host":"frontend.example.test"}},"security":"tls","tlsSettings":{{"serverName":"edge.example.test"}}}}}}"#
    );
    let legacy = format!(
        r#"{{"outbounds":[{{"tag":"Fixture","protocol":"vmess","settings":{{"vnext":[{{"address":"localhost","port":7443,"users":[{{"id":"{ID}","alterId":0,"security":"aes-128-gcm"}}]}}]}},"streamSettings":{{"network":"ws","security":"tls","wsSettings":{{"path":"/vmess","headers":{{"Host":"frontend.example.test"}}}},"tlsSettings":{{"serverName":"edge.example.test","allowInsecure":false}}}}}}],"log":{{}},"inbounds":[]}}"#
    );
    let expected = normalize(&modern).expect("flat Xray VMess");
    assert_eq!(normalize(&legacy), Ok(expected.clone()));
    assert_eq!(
        normalize(URL_SAFE_NO_PAD.encode(legacy)),
        Ok(expected.clone())
    );
    assert_eq!(
        expected[0].transport,
        ProxyTransport::WebSocket {
            path: "/vmess".to_owned(),
            host: Some("frontend.example.test".to_owned()),
            early_data: None,
        },
    );
    assert_eq!(
        expected[0].security,
        ProxySecurity::Tls {
            server_name: "edge.example.test".to_owned(),
            allow_insecure: false,
            alpn: AlpnProtocols::default(),
            fingerprint: None,
        }
    );
}

#[test]
fn sing_box_outbound_list_and_share_link_map_the_same_vless_reality_fields() {
    let key = URL_SAFE_NO_PAD.encode([42; 32]);
    let outbound = format!(
        r#"{{"type":"vless","tag":"Fixture","server":"2001:db8::1","server_port":7443,"uuid":"{ID}","flow":"xtls-rprx-vision","packet_encoding":"xudp","multiplex":{{"enabled":false}},"tls":{{"enabled":true,"server_name":"tls.example.test","utls":{{"enabled":false}},"reality":{{"enabled":true,"public_key":"{key}","short_id":"ABCD"}}}}}}"#
    );
    let link = format!(
        "vless://{ID}@[2001:db8::1]:7443?type=raw&encryption=none&flow=xtls-rprx-vision&security=reality&sni=tls.example.test&pbk={key}&sid=ABCD#Fixture"
    );
    let expected = normalize(&link).expect("basic REALITY URI");
    assert_eq!(normalize(&outbound), Ok(expected.clone()));
    assert_eq!(normalize(format!("[{outbound}]")), Ok(expected.clone()));
    assert_eq!(
        normalize(STANDARD.encode(format!(r#"{{"outbounds":[{outbound}]}}"#))),
        Ok(expected.clone())
    );
    assert_eq!(expected[0].host, "2001:db8::1");
    assert!(matches!(
        expected[0].protocol,
        ProxyProtocol::Vless { vision: true, .. }
    ));
    assert!(
        matches!(&expected[0].security, ProxySecurity::Reality { short_id, .. } if short_id == "abcd")
    );
}

#[test]
fn vless_encryption_is_preserved_by_xray_uri_and_base64() {
    let x25519 = URL_SAFE_NO_PAD.encode([42; 32]);
    let mlkem = URL_SAFE_NO_PAD.encode([37; 1184]);
    let encryption = format!("mlkem768x25519plus.xorpub.1rtt.100-35-50.{x25519}.{mlkem}");
    let modern = serde_json::json!({
        "protocol":"vless","tag":"Encrypted",
        "settings":{"address":"localhost","port":7443,"id":ID,"encryption":encryption,"flow":"xtls-rprx-vision"}
    });
    let legacy = serde_json::json!({"outbounds":[{
        "protocol":"vless","tag":"Encrypted",
        "settings":{"vnext":[{"address":"localhost","port":7443,"users":[{
            "id":ID,"encryption":encryption,"flow":"xtls-rprx-vision"
        }]}]}
    }]});
    let uri = format!(
        "vless://{ID}@localhost:7443?encryption={encryption}&flow=xtls-rprx-vision#Encrypted"
    );
    let profiles = normalize(modern.to_string()).unwrap();
    assert_eq!(
        normalize(STANDARD.encode(legacy.to_string())),
        Ok(profiles.clone())
    );
    assert_eq!(normalize(URL_SAFE_NO_PAD.encode(uri)), Ok(profiles.clone()));
    let protocol = &profiles[0].protocol;
    assert!(matches!(
        protocol,
        ProxyProtocol::Vless { vision: true, encryption: Some(setting), .. }
            if setting.expose() == encryption
    ));
    assert!(!format!("{protocol:?}").contains(&encryption));
}

#[test]
fn v2rayn_and_xray_vmess_share_links_preserve_aead_and_ws() {
    let body = format!(
        r#"{{"v":"2","ps":"Fixture","add":"localhost","port":"7443","id":"{ID}","aid":"0","scy":"auto","net":"ws","type":"none","host":"ws.example.test","path":"/via","tls":"tls","sni":"tls.example.test","alpn":"","fp":""}}"#
    );
    let legacy = format!("vmess://{}", STANDARD.encode(body));
    let uri = format!(
        "vmess://{ID}@localhost:7443?encryption=auto&type=ws&host=ws.example.test&path=%2Fvia&security=tls&sni=tls.example.test#Fixture"
    );
    let first = normalize(legacy).expect("v2rayN legacy VMess");
    assert_eq!(normalize(uri), Ok(first));
}

#[test]
fn tls_alpn_preserves_order_across_json_links_and_base64() {
    let xray = serde_json::json!({
        "tag":"Fixture", "protocol":"vmess", "settings":{"address":"localhost", "port":7443, "id":ID},
        "streamSettings":{"security":"tls", "tlsSettings":{"alpn":["h2","http/1.1"]}}
    });
    let sing_box = serde_json::json!({
        "tag":"Fixture", "type":"vmess", "server":"localhost", "server_port":7443, "uuid":ID,
        "tls":{"enabled":true,"alpn":["h2","http/1.1"],"reality":{"enabled":false}}
    });
    let v2rayn = serde_json::json!({
        "ps":"Fixture", "add":"localhost", "port":7443, "id":ID,
        "net":"tcp", "tls":"tls", "alpn":"h2,http/1.1"
    });
    let direct = format!("vmess://{ID}@localhost:7443?security=tls&alpn=h2%2Chttp%2F1.1#Fixture");
    let expected = normalize(direct).expect("VMess TLS share link with ALPN");
    assert!(matches!(
        &expected[0].security,
        ProxySecurity::Tls { alpn, .. } if alpn.as_slice() == ["h2", "http/1.1"]
    ));
    for input in [
        xray.to_string(),
        URL_SAFE_NO_PAD.encode(sing_box.to_string()),
        format!("vmess://{}", STANDARD.encode(v2rayn.to_string())),
    ] {
        assert_eq!(normalize(input), Ok(expected.clone()));
    }
}

#[test]
fn tls_alpn_rejects_invalid_values_and_other_security_modes() {
    for invalid in [
        serde_json::json!("http/1.1"),
        serde_json::json!([false]),
        serde_json::json!([""]),
        serde_json::json!(["x".repeat(256)]),
    ] {
        let outbound = serde_json::json!({"type":"http","server":"localhost","server_port":443,"tls":{"enabled":true,"alpn":invalid}});
        assert_eq!(
            normalize(outbound.to_string()),
            Err(ClientError::InvalidSubscription)
        );
    }
    let key = URL_SAFE_NO_PAD.encode([42; 32]);
    for link in [
        "http://localhost:8080?alpn=http%2F1.1".to_owned(),
        format!("vless://{ID}@localhost:7443?security=reality&pbk={key}&alpn=h2"),
    ] {
        assert_eq!(normalize(link), Err(ClientError::UnsupportedFeature));
    }
}

#[test]
fn xhttp_json_and_base64_share_links_preserve_modes_authority_and_query() {
    for mode in [
        XhttpMode::Auto,
        XhttpMode::PacketUp,
        XhttpMode::StreamUp,
        XhttpMode::StreamOne,
    ] {
        let json = serde_json::json!({
            "tag":"Fixture", "protocol":"vmess", "settings":{"address":"localhost","port":7443,"id":ID},
            "streamSettings":{"network":"splithttp","splithttpSettings":{"path":"/sessions?trace=1","host":"gateway.example.test:8443","mode":mode.name()}}
        });
        let link = format!(
            "vmess://{ID}@localhost:7443?type=xhttp&path=%2Fsessions%3Ftrace%3D1&host=gateway.example.test%3A8443&mode={}#Fixture",
            mode.name()
        );
        let v2rayn = serde_json::json!({"ps":"Fixture","add":"localhost","port":"7443","id":ID,"net":"xhttp","path":"/sessions?trace=1","host":"gateway.example.test:8443","mode":mode.name()});
        let profiles = normalize(URL_SAFE_NO_PAD.encode(json.to_string())).unwrap();
        assert_eq!(
            profiles[0].transport,
            ProxyTransport::Xhttp {
                path: "/sessions?trace=1".into(),
                host: Some("gateway.example.test:8443".into()),
                mode,
                metadata: XhttpMetadata::default(),
                packet: Default::default(),
            }
        );
        assert_eq!(normalize(link), Ok(profiles.clone()));
        assert_eq!(
            normalize(format!("vmess://{}", STANDARD.encode(v2rayn.to_string()))),
            Ok(profiles)
        );
    }
}

#[test]
fn xhttp_session_and_sequence_metadata_import_across_formats() {
    use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};

    let extra = serde_json::json!({
        "sessionIDPlacement":"header","sessionIDKey":"X-Visit",
        "seqPlacement":"query","seqKey":"p",
        "scMaxEachPostBytes":"128000-256000","scMinPostsIntervalMs":"5-15","uplinkHTTPMethod":"post"
    });
    let uri_extra = utf8_percent_encode(&extra.to_string(), NON_ALPHANUMERIC).to_string();
    let xray = serde_json::json!({
        "protocol":"vless","settings":{"address":"localhost","port":7443,"id":ID},
        "streamSettings":{"network":"xhttp","xhttpSettings":{"mode":"packet-up","extra":extra}}
    });
    let sing_box = serde_json::json!({
        "type":"vless","server":"localhost","server_port":7443,"uuid":ID,
        "transport":{"type":"xhttp","mode":"packet-up","session_placement":"header",
            "session_key":"X-Visit","seq_placement":"query","seq_key":"p",
            "sc_max_each_post_bytes":"128000-256000","sc_min_posts_interval_ms":"5-15",
            "uplink_http_method":"POST"}
    });
    let v2rayn = serde_json::json!({
        "add":"localhost","port":"7443","id":ID,"net":"splithttp","mode":"packet-up",
        "session_placement":"header","session_key":"X-Visit","seqPlacement":"query","seqKey":"p",
        "scMaxEachPostBytes":"128000-256000","scMinPostsIntervalMs":"5-15","uplinkHTTPMethod":""
    });
    let expected = ProxyTransport::Xhttp {
        path: "/".into(),
        host: None,
        mode: XhttpMode::PacketUp,
        metadata: XhttpMetadata {
            session_id_placement: XhttpPlacement::Header,
            session_id_key: Some("X-Visit".into()),
            seq_placement: XhttpPlacement::Query,
            seq_key: Some("p".into()),
        },
        packet: XhttpPacketOptions {
            max_post_bytes: Some(XhttpPacketRange::new(128_000, 256_000).unwrap()),
            min_post_interval_ms: Some(XhttpPacketRange::new(5, 15).unwrap()),
            ..Default::default()
        },
    };
    for input in [
        format!("vless://{ID}@localhost:7443?type=xhttp&mode=packet-up&extra={uri_extra}"),
        format!(
            "vless://{ID}@localhost:7443?type=xhttp&mode=packet-up&sessionPlacement=header&sessionKey=X-Visit&seqPlacement=query&seqKey=p&scMaxEachPostBytes=128000-256000&scMinPostsIntervalMs=5-15&uplinkHTTPMethod=POST"
        ),
        xray.to_string(),
        STANDARD.encode(xray.to_string()),
        sing_box.to_string(),
        format!("vmess://{}", STANDARD.encode(v2rayn.to_string())),
    ] {
        assert_eq!(normalize(input).unwrap()[0].transport, expected);
    }
}

#[test]
fn xhttp_rejects_ambiguous_and_invalid_metadata() {
    for options in [
        "type=xhttp&seqPlacement=query&seq_placement=query",
        "type=xhttp&sessionIDKey=a&sessionKey=b",
        "type=xhttp&seqPlacement=query&extra=%7B%7D",
        "type=xhttp&seqPlacement=body",
        "type=xhttp&seqPlacement=header&seqKey=Content-Length",
        "type=xhttp&sessionIDPlacement=header&seqPlacement=header&seqKey=x-session",
        "type=xhttp&seqPlacement=cookie&seqKey=bad%3Dcookie",
        "type=xhttp&seqPlacement=query&seqKey=bad%0Dkey",
        "type=ws&seqPlacement=header",
        "type=ws&scMaxEachPostBytes=128000",
        "type=xhttp&scMaxEachPostBytes=256000-128000",
        "type=xhttp&scMinPostsIntervalMs=0-15",
        "type=xhttp&scMinPostsIntervalMs=-1",
        "type=xhttp&scMaxEachPostBytes=2147483648",
        "type=xhttp&scMinPostsIntervalMs=5&sc_min_posts_interval_ms=10",
    ] {
        assert!(normalize(format!("vless://{ID}@localhost:7443?{options}")).is_err());
    }
    let conflicting = serde_json::json!({
        "protocol":"vless","settings":{"address":"localhost","port":7443,"id":ID},
        "streamSettings":{"network":"xhttp","xhttpSettings":{
            "extra":{"seqPlacement":"query","seq_placement":"header"}
        }}
    });
    assert_eq!(
        normalize(conflicting.to_string()),
        Err(ClientError::InvalidSubscription)
    );
}

#[test]
fn fetched_xray_wrapper_imports_get_packet_up_and_keeps_unsupported_policies_separate() {
    use vpn_application::client::ProfileUnavailableReason;

    let link =
        format!("vless://{ID}@localhost:7443?type=xhttp&mode=stream-up&uplinkHTTPMethod=GET");
    assert_eq!(
        normalize(link),
        Err(ClientError::UnsupportedXhttpUploadMethod)
    );
    let mut document = serde_json::json!({
        "remarks":"Fixture",
        "dns":{"servers":["192.0.2.53"]},
        "routing":{"domainStrategy":"AsIs","rules":[]},
        "outbounds":[{
            "protocol":"vless","tag":"proxy",
            "settings":{"address":"localhost","port":7443,"id":ID},
            "streamSettings":{
                "network":"xhttp","security":"tls",
                "tlsSettings":{"fingerprint":"firefox","alpn":["h2"]},
                "xhttpSettings":{"mode":"packet-up","extra":{
                    "seqPlacement":"header","scMaxEachPostBytes":"128000-256000",
                    "scMinPostsIntervalMs":"5-15","uplinkHTTPMethod":"GET",
                    "sessionIDLength":"18-24","sessionIDTable":"base36",
                    "xPaddingBytes":"100-1000","xPaddingMethod":"tokenish",
                    "xPaddingObfsMode":true,"xPaddingPlacement":"queryInHeader",
                    "xPaddingHeader":"Referer","xPaddingKey":"padding",
                    "xmux":{"maxConnections":"3-3","hMaxRequestTimes":"50-100"}
                }}
            }
        },{"protocol":"freedom","tag":"direct"},{"protocol":"blackhole","tag":"blocked"}]
    });
    let normalize_wrapper = |document: &serde_json::Value| {
        SubscriptionNormalizer
            .normalize_subscription(document.to_string().as_bytes())
            .unwrap()
    };
    let imported = normalize_wrapper(&document);
    assert!(imported.unavailable_profiles.is_empty());
    let profile = &imported.profiles[0];
    assert_eq!(profile.name, "Fixture");
    assert_eq!(
        profile.security.fingerprint().unwrap().name(),
        "firefox-148"
    );
    let ProxyTransport::Xhttp { packet, .. } = &profile.transport else {
        panic!("expected XHTTP transport");
    };
    let rendered = crate::xhttp_wire::render(&packet.wire);
    assert_eq!(rendered["uplink_http_method"], "GET");
    assert_eq!(rendered["session_id_length"], "18-24");
    assert_eq!(rendered["x_padding_bytes"], "100-1000");
    assert_eq!(rendered["xmux"]["max_connections"], 3);
    assert_eq!(crate::xhttp_wire::restore(&rendered).unwrap(), packet.wire);
    assert_eq!(
        normalize(profile.source.as_ref().unwrap().expose()).unwrap()[0].transport,
        profile.transport
    );

    document["routing"]["rules"] = serde_json::json!([{"outboundTag":"blocked","domain":["test"]}]);
    let result = normalize_wrapper(&document);
    assert!(result.profiles.is_empty());
    let row = &result.unavailable_profiles[0];
    assert_eq!(row.reason, ProfileUnavailableReason::Unsupported);
    assert_eq!(
        row.probe_endpoint().map(|endpoint| endpoint.address()),
        Some(("localhost", 7443))
    );
}

#[test]
fn xhttp_rejects_unmapped_transport_options() {
    for options in [
        "type=xhttp&mode=other",
        "type=xhttp&extra=%7B%22downloadSettings%22%3A%7B%7D%7D",
        "type=ws&mode=stream-one",
    ] {
        assert_eq!(
            normalize(format!("vless://{ID}@localhost:7443?{options}")),
            Err(ClientError::UnsupportedFeature)
        );
    }
}

#[test]
fn xhttp_preserves_protocol_and_tls_settings_for_engine_validation() {
    let profiles = normalize(format!(
        "vless://{ID}@localhost:7443?type=xhttp&security=tls&flow=xtls-rprx-vision&alpn=h2%2Chttp%2F1.1"
    ))
    .expect("normalize settings independently of the engine");
    let profile = &profiles[0];
    assert!(matches!(profile.transport, ProxyTransport::Xhttp { .. }));
    assert!(matches!(
        profile.protocol,
        ProxyProtocol::Vless { vision: true, .. }
    ));
    assert!(matches!(
        &profile.security,
        ProxySecurity::Tls { alpn, .. } if alpn.as_slice() == ["h2", "http/1.1"]
    ));
}

#[test]
fn happ_socks_forms_preserve_wire_credentials_and_ipv6() {
    let credentials = "fixture:literal%25:/?@+";
    let userinfo = STANDARD.encode(credentials);
    let authority = URL_SAFE_NO_PAD.encode(format!("{credentials}@[::1]:21080"));
    let direct = "socks5://fixture:literal%2525%3A%2F%3F%40%2B@[::1]:21080#Example";
    let inner = STANDARD.encode(direct.strip_suffix("#Example").unwrap());
    for link in [
        direct.to_owned(),
        format!("socks://{userinfo}@[::1]:21080#Example"),
        format!("socks://{authority}#Example"),
        format!("socks://{inner}#Example"),
    ] {
        assert_eq!(normalize(link).unwrap(), normalize(direct).unwrap());
    }
    for encoded in [
        STANDARD.encode("http://fixture:secret@localhost:8080"),
        STANDARD.encode("fixture:secret@localhost:1080?security=tls"),
        STANDARD.encode("socks://fixture:secret@local\thost:1080"),
    ] {
        assert!(normalize(format!("socks://{encoded}#Example")).is_err());
    }
    assert!(normalize("socks://localhost:1080").is_ok());
    assert!(normalize("socks://invalid@localhost:1080").is_err());
}

#[test]
fn wrapped_json_subscription_preserves_order_and_checks_total_profile_count() {
    let leaf = |name: &str| {
        serde_json::json!({
            "protocol":"socks", "tag":name, "settings":{"address":"localhost","port":1080}
        })
    };
    let wrapper = |outbounds| serde_json::json!({"outbounds":outbounds,"inbounds":[],"log":{}});
    let source = serde_json::json!([
        wrapper(vec![leaf("First"), leaf("Second")]),
        "socks5://localhost:1081#Third",
        wrapper(vec![leaf("Fourth")])
    ]);
    let encoded = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&source).unwrap());
    let profiles = normalize(encoded).unwrap();
    let names: Vec<_> = profiles
        .iter()
        .map(|profile| profile.name.as_str())
        .collect();
    assert_eq!(names, ["First", "Second", "Third", "Fourth"]);

    let full = wrapper(vec![leaf("One"); MAX_PROFILES]);
    let overflow = serde_json::json!([full, wrapper(vec![leaf("Two")])]);
    assert_eq!(
        normalize(overflow.to_string()),
        Err(ClientError::PayloadTooLarge)
    );
    let routed = serde_json::json!([source[0].clone(), {"outbounds":[leaf("Route")],"routing":{"rules":[{}]}}]);
    assert_eq!(
        normalize(routed.to_string()),
        Err(ClientError::InvalidSubscription)
    );
}

#[test]
fn imports_xray_server_and_the_simple_blazecore_export() {
    let xray = br#"{
        "log":{"loglevel":"warning"},
        "inbounds":[{"port":1080,"protocol":"socks"}],
        "outbounds":[
            {"tag":"proxy","protocol":"socks","settings":{"servers":[{"address":"example.test","port":1081}]}},
            {"tag":"direct","protocol":"freedom","settings":{}},
            {"tag":"block","protocol":"blackhole","settings":{}}
        ]
    }"#;
    let blazecore = br#"[{
        "address":"127.0.0.1:1080","protocol":{"type":"mixed"},
        "rules":[{"action":"allow","masks":["0.0.0.0/0","::/0"],
            "client_chain":{"address":"example.test:1081","protocol":{"type":"socks"}}}]
    }]"#;
    for document in [xray.as_slice(), blazecore.as_slice()] {
        let [server]: [ProxyProfile; 1] = SubscriptionNormalizer
            .normalize(document)
            .unwrap()
            .try_into()
            .unwrap();
        assert_eq!(server.host, "example.test");
        assert_eq!(server.port, 1081);
        assert!(matches!(server.protocol, ProxyProtocol::Socks5 { .. }));
    }
}

#[test]
fn shadowsocks_legacy_and_sip002_preserve_percent_in_passwords() {
    let credentials = "aes-128-gcm:fixture%25:pass";
    let old = STANDARD.encode(format!("{credentials}@localhost:8388"));
    let current = URL_SAFE_NO_PAD.encode(credentials);
    let inputs = [
        format!("ss://{old}#Fixture"),
        format!("ss://{current}@localhost:8388/#Fixture"),
        "ss://aes-128-gcm:fixture%2525%3Apass@localhost:8388#Fixture".to_owned(),
    ];
    for input in inputs {
        let [profile]: [ProxyProfile; 1] = normalize(input)
            .expect("import Shadowsocks")
            .try_into()
            .unwrap();
        assert!(matches!(
            profile.protocol,
            ProxyProtocol::Shadowsocks { password, .. }
                if password.expose() == "fixture%25:pass"
        ));
    }
}

#[test]
fn websocket_early_data_preserves_xray_and_sing_box_carriers() {
    let xray = serde_json::json!({
        "protocol":"vmess","tag":"fixture","settings":{"address":"127.0.0.1","port":443,"id":ID},
        "streamSettings":{"network":"ws","security":"tls","tlsSettings":{"serverName":"edge.example.test"},
            "wsSettings":{"path":"/ws?token=a%2Bb&e%64=2560"}}
    });
    let xray = normalize(xray.to_string()).unwrap().remove(0);
    let v2ray = serde_json::json!({
        "v":"2","ps":"fixture","add":"127.0.0.1","port":"443","id":ID,
        "net":"ws","tls":"tls","sni":"edge.example.test","path":"/ws?token=a%2Bb&ed=2560"
    });
    let v2ray = normalize(format!("vmess://{}", STANDARD.encode(v2ray.to_string()))).unwrap();
    assert_eq!(v2ray[0], xray);
    assert_eq!(
        xray.transport,
        ProxyTransport::WebSocket {
            path: "/ws?token=a%2Bb".into(),
            host: Some("edge.example.test".into()),
            early_data: Some(WebSocketEarlyData {
                max_bytes: NonZeroU16::new(2560).unwrap(),
                header_name: Some("sec-websocket-protocol".into()),
            }),
        }
    );
    let mut sing_box = serde_json::json!({
        "type":"vless","server":"::1","server_port":7443,"uuid":ID,
        "transport":{"type":"ws","path":"/ws","headers":{"Host":""},"max_early_data":2048}
    });
    for header in [None, Some("X-Initial-Data")] {
        if let Some(header) = header {
            sing_box["transport"]["early_data_header_name"] = serde_json::json!(header);
        }
        let encoded = URL_SAFE_NO_PAD.encode(sing_box.to_string());
        let profile = normalize(encoded).unwrap().remove(0);
        assert_eq!(
            profile.transport,
            ProxyTransport::WebSocket {
                path: "/ws".into(),
                host: Some("[::1]:7443".into()),
                early_data: Some(WebSocketEarlyData {
                    max_bytes: NonZeroU16::new(2048).unwrap(),
                    header_name: header.map(str::to_ascii_lowercase),
                }),
            }
        );
    }
    sing_box["transport"]["max_early_data"] = serde_json::json!("0");
    sing_box["transport"]["early_data_header_name"] = serde_json::json!("");
    assert!(matches!(
        normalize(sing_box.to_string()).unwrap()[0].transport,
        ProxyTransport::WebSocket {
            early_data: None,
            ..
        }
    ));
}

#[test]
fn malformed_or_unmapped_security_transport_and_routing_fail_closed() {
    let rejected = [
        r#"{"protocol":"http","settings":{"address":"localhost","port":80},"streamSettings":{"security":true}}"#,
        r#"{"protocol":"http","settings":{"address":"localhost","port":80},"streamSettings":{"security":"none","tlsSettings":{"serverName":"example.test"}}}"#,
        r#"{"protocol":"http","settings":{"address":"localhost","port":80},"streamSettings":{"security":"tls","tlsSettings":{"pinnedPeerCertificateChainSha256":["pin"]}}}"#,
        r#"{"protocol":"http","settings":{"address":"localhost","address":"example.test","port":80}}"#,
        r#"{"protocol":"http","settings":{"address":"localhost","port":80},"streamSettings":{"method":"raw","rawSettings":{"header":{"type":"http"}}}}"#,
        r#"{"protocol":"http","settings":{"address":"localhost","port":80},"streamSettings":{"network":"ws","wsSettings":{"path":"/?ed=2048&e%64=0"}}}"#,
        r#"{"protocol":"http","settings":{"address":"localhost","port":80},"streamSettings":{"network":"ws","wsSettings":{"path":"/?ed=-1"}}}"#,
        r#"{"type":"socks","server":"localhost","server_port":1080,"detour":"other"}"#,
        r#"{"type":"socks","server":"localhost","server_port":1080,"network":"tcp"}"#,
        r#"{"type":"http","server":"localhost","server_port":443,"tls":{"enabled":true,"utls":{"enabled":true,"fingerprint":"chrome"}}}"#,
        r#"{"type":"http","server":"localhost","server_port":443,"tls":{"enabled":false,"server_name":"tls.example.test"}}"#,
        r#"{"type":"http","server":"localhost","server_port":443,"tls":{"enabled":true,"disable_sni":true}}"#,
        r#"{"outbounds":[{"type":"socks","server":"localhost","server_port":1080}],"route":{"final":"direct"}}"#,
        r#"{"outbounds":[{"type":"socks","server":"localhost","server_port":1080},{"type":"direct"}]}"#,
        "https://localhost:7443?security=none",
        "http://localhost:8080?security=tls&%73ecurity=none",
        "http://localhost:8080?path=/unexpected",
        "http://localhost:8080?security=tls&sni=bad%GGname",
        "http://user%3Aalias:fixture@localhost:8080",
        r#"{"type":"http","server":"localhost","server_port":8080,"username":"fixture","password":"unsafe\rvalue"}"#,
        "ss://aes-128-gcm:fixture@localhost:8388?plugin=obfs-local",
    ];
    for (index, case) in rejected.into_iter().enumerate() {
        assert!(normalize(case).is_err(), "negative fixture {index}");
    }
}

#[test]
fn hysteria2_links_and_sing_box_preserve_auth_tls_and_salamander() {
    let uri = "hy2://fixture-user:fixture%3Apass@localhost/?sni=edge.example.test&obfs=salamander&obfs-password=shared%26secret#Hysteria2";
    let outbound = serde_json::json!({
        "type":"hysteria2","tag":"Hysteria2","server":"localhost","server_port":443,
        "password":"fixture-user:fixture:pass","tls":{"enabled":true,"server_name":"edge.example.test"},
        "obfs":{"type":"salamander","password":"shared&secret"}
    });
    let expected = normalize(uri).unwrap();
    assert_eq!(
        normalize(STANDARD.encode(outbound.to_string())),
        Ok(expected.clone())
    );
    assert_eq!(
        normalize(uri.replace("hy2://", "hysteria2://")),
        Ok(expected.clone())
    );
    let profile = &expected[0];
    assert!(matches!(
        &profile.protocol,
        ProxyProtocol::Hysteria2 { password, salamander_password: Some(secret), udp_enabled: true }
            if password.expose() == "fixture-user:fixture:pass" && secret.expose() == "shared&secret"
    ));
    let pin = "AB:".repeat(31) + "AB";
    let pinned = normalize(format!(
        "hy2://fixture@localhost?insecure=1&pinSHA256={pin}"
    ))
    .unwrap();
    assert_eq!(
        pinned[0].transport,
        ProxyTransport::Quic {
            certificate_sha256: Some("ab".repeat(32))
        }
    );
    assert!(matches!(
        pinned[0].security,
        ProxySecurity::Tls {
            allow_insecure: true,
            ..
        }
    ));
}

#[test]
fn hysteria2_rejects_options_that_cannot_be_mapped() {
    for uri in [
        "hy2://fixture@localhost?obfs=gecko&obfs-password=shared",
        "hy2://fixture@localhost?obfs=salamander",
        "hy2://fixture@localhost?pinSHA256=wrong",
        "hysteria2://fixture@localhost?ech=unknown",
        "hy2://fixture@localhost:443,20000-30000",
    ] {
        assert!(normalize(uri).is_err());
    }
    let base = serde_json::json!({
        "type":"hysteria2","server":"localhost","server_port":443,"password":"fixture",
        "tls":{"enabled":true}
    });
    for (key, value) in [
        ("network", serde_json::json!("udp")),
        ("up_mbps", serde_json::json!(100)),
        ("server_ports", serde_json::json!(["20000:30000"])),
        ("tls", serde_json::json!({"enabled":false})),
    ] {
        let mut outbound = base.clone();
        outbound[key] = value;
        assert_eq!(
            normalize(outbound.to_string()),
            Err(ClientError::UnsupportedFeature)
        );
    }
}

#[test]
fn empty_json_tls_server_name_preserves_the_endpoint_name_default() {
    let xray = r#"{"protocol":"http","settings":{"address":"localhost","port":443},"streamSettings":{"security":"tls","tlsSettings":{"serverName":""}}}"#;
    let sing_box = r#"{"type":"http","server":"localhost","server_port":443,"tls":{"enabled":true,"server_name":""}}"#;
    let profiles = normalize(xray).unwrap();
    assert_eq!(normalize(sing_box), Ok(profiles.clone()));
    assert_eq!(
        profiles[0].security,
        ProxySecurity::Tls {
            server_name: "localhost".to_owned(),
            allow_insecure: false,
            alpn: AlpnProtocols::default(),
            fingerprint: None,
        },
    );
}

#[test]
fn lists_preserve_order_and_enforce_profile_count() {
    let link = "socks5://localhost:1080";
    let list =
        format!("[\"{link}\",{{\"type\":\"http\",\"server\":\"localhost\",\"server_port\":80}}]");
    let imported = normalize(list).expect("JSON URI and outbound list");
    assert_eq!(imported[0].protocol.name(), "SOCKS5");
    assert_eq!(imported[1].protocol.name(), "HTTP");
    let repeated = format!("{link}\n").repeat(MAX_PROFILES);
    assert_eq!(normalize(&repeated).unwrap().len(), MAX_PROFILES);
    let too_many = format!("{repeated}{link}");
    assert_eq!(normalize(too_many), Err(ClientError::PayloadTooLarge));
}
