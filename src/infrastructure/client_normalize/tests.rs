use super::*;

const ID: &str = "a5c41b77-fc31-4ab0-a275-1c67aa9aab31";

fn normalize(input: impl AsRef<[u8]>) -> Result<Vec<ProxyProfile>, ClientError> {
    SubscriptionNormalizer.normalize(input.as_ref())
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
                mode
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
        Err(ClientError::UnsupportedFeature)
    );
}

#[test]
fn imports_xray_server_and_the_simple_shoes_export() {
    let xray = br#"{
        "log":{"loglevel":"warning"},
        "inbounds":[{"port":1080,"protocol":"socks"}],
        "outbounds":[
            {"tag":"proxy","protocol":"socks","settings":{"servers":[{"address":"example.test","port":1081}]}},
            {"tag":"direct","protocol":"freedom","settings":{}},
            {"tag":"block","protocol":"blackhole","settings":{}}
        ]
    }"#;
    let shoes = br#"[{
        "address":"127.0.0.1:1080","protocol":{"type":"mixed"},
        "rules":[{"action":"allow","masks":["0.0.0.0/0","::/0"],
            "client_chain":{"address":"example.test:1081","protocol":{"type":"socks"}}}]
    }]"#;
    for document in [xray.as_slice(), shoes.as_slice()] {
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
