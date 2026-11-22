#![cfg(windows)]

use std::fs;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use client_core::{
    ClientCommand, ClientCoreOptions, ClientError, ClientEventKind, ClientHandle, ClientSnapshot,
    ConnectionMode, ConnectionStatus, ModeCapabilities, ProfileLatency, ProfileUnavailableReason,
    ProviderChangeKind, ProviderKey, RefreshStatus, RequestId, Secret, SubmitError,
    SubscriptionRefreshMode,
};
use client_core::{
    DnsPolicy, RouteAction, RouteRule, RouteTarget, RoutingAssignment, RoutingInput, RoutingProfile,
};
use vpn_application::client::{
    Cancellation, ClientEnginePort, EngineRuntime, ProxyProfile, RunningConnection, StateStorePort,
};
use vpn_infrastructure::client_store::{SealedCatalogStore, UserSecretProtection};

const SOCKET_TIMEOUT: Duration = Duration::from_secs(5);
const BACKEND_TIMEOUT: Duration = Duration::from_secs(45);
const SUBSCRIPTION_PATH: &str = "/subscription?key=synthetic-url-token";
const PROFILE_PASSWORD: &str = "synthetic-profile-password";
const ECHO_PAYLOAD: &[u8] = b"blazer\x00echo\xff";
const ENGINE_INCOMPATIBLE_XHTTP: &str = "vless://a5c41b77-fc31-4ab0-a275-1c67aa9aab31@localhost:7443?type=xhttp&security=tls&flow=xtls-rprx-vision#Engine-incompatible";

#[test]
fn profile_source_edit_updates_the_selected_profile_and_persists_after_reload() {
    let directory = TestDirectory::new();
    let options = directory.options();
    let mut backend = TestBackend::start(options.clone());
    let original = "socks5://user:fixture-password@127.0.0.1:18001#Original";
    let added = backend.run(ClientCommand::ImportText {
        title: "Original".into(),
        text: original.into(),
    });
    let profile_uuid = added.profiles[0].profile_uuid.clone();
    assert_eq!(
        added.profiles[0].source.as_ref().unwrap().expose(),
        original
    );
    assert!(!format!("{added:?}").contains(original));
    let update = |text| ClientCommand::UpdateProfile {
        profile_uuid: profile_uuid.clone(),
        text: Secret::new(text),
    };
    let request = backend.send(update("not a valid config"));
    let invalid = backend.wait(Some(request), "reject invalid profile source");
    assert!(invalid.last_error.is_some());
    assert_eq!(
        invalid.profiles[0].source.as_ref().unwrap().expose(),
        original
    );
    let outbound = r#"{"type":"socks","tag":"Edited","server":"127.0.0.1","server_port":18002}"#;
    let saved = backend.run(update(outbound));
    assert_eq!(saved.last_error, None);
    assert_eq!(saved.profiles.len(), 1);
    assert_eq!(saved.profiles[0].profile_uuid, profile_uuid);
    assert_eq!(
        saved.selected_profile_uuid.as_deref(),
        Some(profile_uuid.as_str())
    );
    assert_eq!(saved.profiles[0].name, "Edited");
    assert_eq!(saved.profiles[0].server, "127.0.0.1:18002");
    let source = saved.profiles[0].source.as_ref().unwrap();
    assert!(source.expose().contains(r#""server_port": 18002"#));
    backend.handle.shutdown().expect("shutdown before reload");
    let mut reopened = TestBackend::start(options);
    let restored = reopened.handle.snapshot();
    assert_eq!(restored.profiles[0].source.as_ref(), Some(source));
    assert_eq!(restored.profiles[0].server, "127.0.0.1:18002");
    reopened.handle.shutdown().expect("shutdown after reload");
}

#[test]
fn add_subscription_exposes_the_http_code_and_server_detail() {
    let directory = TestDirectory::new();
    let source = loopback_listener();
    let backend = TestBackend::start(directory.options());
    for (status_line, status, body, detail) in [
        (
            "403 Forbidden",
            403,
            r#"{"detail":"HWID header required"}"#,
            "HWID header required",
        ),
        (
            "422 Unprocessable Entity",
            422,
            r#"{"message":"Subscription validation failed"}"#,
            "Subscription validation failed",
        ),
    ] {
        let request = backend.send(ClientCommand::AddSubscription {
            title: "HTTP error fixture".into(),
            url: format!("http://{}{SUBSCRIPTION_PATH}", source.local_addr().unwrap()),
        });
        serve_provider_response(
            &source,
            SUBSCRIPTION_PATH,
            status_line,
            &[("Content-Type", "application/json")],
            body,
        );
        let snapshot = backend.wait(Some(request), "provider rejects the subscription");
        assert_subscription_http(snapshot.last_error, status, detail);
        assert!(matches!(
            snapshot.diagnostic_events.last().map(|event| event.kind),
            Some(ClientEventKind::SubscriptionFailed(
                ClientError::SubscriptionHttp(_)
            ))
        ));
        assert!(!format!("{:?}", snapshot.last_error).contains(detail));
        assert!(snapshot.subscriptions.is_empty());
    }
}

fn assert_subscription_http(error: Option<ClientError>, status: u16, detail: &str) {
    let Some(ClientError::SubscriptionHttp(response)) = error else {
        panic!("expected a subscription HTTP error, got {error:?}");
    };
    assert_eq!(response.status(), status);
    assert_eq!(response.detail(), Some(detail));
}

#[test]
fn subscription_hwid_survives_refused_import_and_is_reused_for_refresh() {
    let directory = TestDirectory::new();
    let source = loopback_listener();
    let mut backend = TestBackend::start(directory.options());
    let (first, rejected) = add_subscription_with_hwid(&backend, &source, "403 Forbidden", "");
    assert_subscription_http(rejected.last_error, 403, "Forbidden");
    assert!(
        rejected
            .subscription_hwid
            .as_ref()
            .map(|hwid| hwid.expose())
            == Some(first.as_str())
    );
    assert!(!format!("{rejected:?}").contains(&first));
    backend.handle.shutdown().unwrap();

    let reopened = TestBackend::start(directory.options());
    let profiles = "socks5://127.0.0.1:12345#Fixture";
    let (second, added) = add_subscription_with_hwid(&reopened, &source, "200 OK", profiles);
    assert_eq!(added.last_error, None);
    assert!(
        first == second,
        "the same installation must retain its HWID"
    );
    assert!(added.subscription_hwid.as_ref().map(|hwid| hwid.expose()) == Some(second.as_str()));

    let request = reopened.send(ClientCommand::RefreshSubscription {
        subscription_uuid: added.subscriptions[0].subscription_uuid.clone(),
    });
    let third = serve_versioned_response(&source, SUBSCRIPTION_PATH, "200 OK", &[], profiles, None)
        .expect("refresh sends HWID");
    assert!(second == third, "refresh must retain the installation HWID");
    assert_eq!(
        reopened.wait(Some(request), "refresh with HWID").last_error,
        None
    );
}

fn add_subscription_with_hwid(
    backend: &TestBackend,
    source: &TcpListener,
    status: &str,
    body: &str,
) -> (String, ClientSnapshot) {
    let request = backend.send(ClientCommand::AddSubscription {
        title: "HWID fixture".into(),
        url: format!("http://{}{SUBSCRIPTION_PATH}", source.local_addr().unwrap()),
    });
    let id = serve_versioned_response(source, SUBSCRIPTION_PATH, status, &[], body, None)
        .expect("import sends HWID");
    assert_eq!(id.len(), 36);
    (id, backend.wait(Some(request), "import with HWID"))
}

#[test]
fn automatic_subscription_title_refreshes_until_manually_renamed() {
    let directory = TestDirectory::new();
    let source = loopback_listener();
    let mut backend = TestBackend::start(directory.options());
    let path = "/subscription?key=synthetic-token&name=Fallback+%F0%9F%8C%8D";
    let profiles = "socks5://127.0.0.1:12345#Fixture";
    let request = backend.send(ClientCommand::AddSubscription {
        title: String::new(),
        url: format!("http://{}{path}", source.local_addr().unwrap()),
    });
    serve_provider_response(
        &source,
        path,
        "200 OK",
        &[
            ("profile-title", "base64:8J+agCBQcm92aWRlcg=="),
            ("name", "Alias"),
        ],
        &format!("#profile-title: Body title\n{profiles}"),
    );
    let added = backend.wait(Some(request), "add automatic title");
    assert_eq!(added.last_error, None);
    assert_eq!(added.subscriptions[0].title, "\u{1f680} Provider");
    assert!(added.subscriptions[0].auto_title);
    let uuid = added.subscriptions[0].subscription_uuid.clone();
    let refreshed = provider_refresh(&backend, &source, &uuid, path, &[], profiles);
    assert_eq!(refreshed.subscriptions[0].title, "Fallback \u{1f30d}");

    backend.run(ClientCommand::RenameSubscription {
        subscription_uuid: uuid.clone(),
        title: "Manual title".into(),
    });
    backend.handle.shutdown().unwrap();

    let reopened = TestBackend::start(directory.options());
    let refreshed = provider_refresh(
        &reopened,
        &source,
        &uuid,
        path,
        &[("name", "Provider renamed")],
        profiles,
    );
    assert_eq!(refreshed.subscriptions[0].title, "Manual title");
    assert!(!refreshed.subscriptions[0].auto_title);
    let cleared = reopened.run(ClientCommand::RenameSubscription {
        subscription_uuid: uuid,
        title: String::new(),
    });
    assert_eq!(cleared.subscriptions[0].title, "Provider renamed");
    assert!(cleared.subscriptions[0].auto_title);
}

#[test]
fn disabled_subscription_hwid_persists_and_omits_the_header() {
    let directory = TestDirectory::new();
    let source = loopback_listener();
    let mut backend = TestBackend::start(directory.options());
    let settings = backend.run(ClientCommand::SetSubscriptionHwid { enabled: false });
    assert_eq!(settings.last_error, None);
    assert!(!settings.send_subscription_hwid);
    backend.handle.shutdown().unwrap();

    let reopened = TestBackend::start(directory.options());
    let request = reopened.send(ClientCommand::AddSubscription {
        title: "Without HWID".into(),
        url: format!("http://{}{SUBSCRIPTION_PATH}", source.local_addr().unwrap()),
    });
    let id = serve_versioned_response(
        &source,
        SUBSCRIPTION_PATH,
        "200 OK",
        &[],
        "socks5://127.0.0.1:12345#Fixture",
        None,
    );
    assert!(id.is_none());
    let added = reopened.wait(Some(request), "import without HWID");
    assert!(!added.send_subscription_hwid);
    assert_eq!(added.last_error, None);
}

#[test]
fn subscription_usage_projects_provider_counters_without_raw_metadata() {
    let directory = TestDirectory::new();
    let server = loopback_listener();
    let mut backend = TestBackend::start(directory.options());
    let added = provider_add(
        &backend,
        &server,
        &[(
            "subscription-userinfo",
            "upload=12; download=30; total=100; expire=1800000000; arbitrary=fixture-secret",
        )],
        &subscription_profiles("Spare"),
    );
    let uuid = added.subscriptions[0].subscription_uuid.clone();
    assert_eq!(
        added.subscription_usage.get(&uuid),
        Some(&client_core::SubscriptionUsage {
            used_bytes: Some(42),
            total_bytes: Some(100),
            expires_at_unix: Some(1_800_000_000),
        }),
    );
    backend.handle.shutdown().unwrap();
    let restarted = TestBackend::start(directory.options());
    let recovered = restarted.wait(None, "reload subscription usage");
    assert_eq!(recovered.subscription_usage, added.subscription_usage);
    let removed = restarted.run(ClientCommand::RemoveSubscription {
        subscription_uuid: uuid,
    });
    assert!(removed.subscription_usage.is_empty());
}

#[test]
fn subscription_notices_are_visible_without_provider_id() {
    let directory = TestDirectory::new();
    let source = loopback_listener();
    let backend = TestBackend::start(directory.options());
    let added = provider_add(
        &backend,
        &source,
        &[
            ("sub-info-text", "Maintenance tonight"),
            ("sub-info-color", "blue"),
            ("sub-info-button-text", "Details"),
            ("sub-info-button-link", "https://example.invalid/status"),
        ],
        "#announce: Welcome\nsocks5://127.0.0.1:12345#Fixture",
    );
    assert_eq!(added.last_error, None);
    let provider = &added.provider_subscriptions[0];
    let display = &provider.display;
    assert_eq!(display.announcement.as_deref(), Some("Welcome"));
    let notice = display.info.as_ref().unwrap();
    assert_eq!(notice.text, "Maintenance tonight");
    assert_eq!(
        notice.button.as_ref().unwrap().url,
        "https://example.invalid/status"
    );
    let renamed = backend.run(ClientCommand::RenameSubscription {
        subscription_uuid: provider.subscription_uuid.clone(),
        title: "Renamed".into(),
    });
    assert_eq!(
        renamed.diagnostic_events.last().map(|event| event.kind),
        Some(ClientEventKind::SubscriptionSettingsSaved)
    );
}

#[test]
#[ignore = "requires BLAZER_TEST_BLAZECORE_EXE pointing to the SHA-256-pinned BlazeCore binary"]
fn saved_local_proxy_port_applies_on_the_next_connection() {
    let directory = TestDirectory::new();
    let source = std::env::var_os("BLAZER_TEST_BLAZECORE_EXE").unwrap();
    let backend = TestBackend::start(ClientCoreOptions {
        engine_path: source.into(),
        ..directory.options()
    });
    backend.run(ClientCommand::ImportText {
        title: "Port fixture".into(),
        text: "socks5://127.0.0.1:1080#Port".into(),
    });
    let first = loopback_listener();
    let next = loopback_listener();
    let first_address = first.local_addr().unwrap();
    let next_address = next.local_addr().unwrap();
    backend.run(ClientCommand::SetLocalProxyPort {
        port: first_address.port(),
    });
    let connect = || ClientCommand::Connect {
        mode: ConnectionMode::LocalProxy,
    };
    drop(first);
    let connected = backend.run(connect());
    assert_eq!(
        connected.local_proxy.as_deref(),
        Some(first_address.to_string().as_str())
    );
    assert_eq!(connected.http_proxy, connected.local_proxy);
    let edited = backend.run(ClientCommand::SetLocalProxyPort {
        port: next_address.port(),
    });
    assert_eq!(edited.local_proxy_port, next_address.port());
    assert_eq!(edited.local_proxy, connected.local_proxy);
    backend.run(ClientCommand::Disconnect);
    drop(next);
    let reconnected = backend.run(connect());
    assert_eq!(
        reconnected.local_proxy.as_deref(),
        Some(next_address.to_string().as_str())
    );
    backend.run(ClientCommand::Disconnect);
}

#[test]
fn subscription_user_agent_uses_blazer_by_default_and_persists_overrides() {
    const CUSTOM: &str = "synthetic-private-agent/1.0 (Windows)";
    const DEFAULT: &str = match option_env!("BLAZER_RELEASE_USER_AGENT") {
        Some(agent) => agent,
        None => concat!(
            "Blazer/",
            env!("CARGO_PKG_VERSION_MAJOR"),
            ".",
            env!("CARGO_PKG_VERSION_MINOR")
        ),
    };
    const ETAG: &str = r#""fixture-version""#;
    let directory = TestDirectory::new();
    let server = loopback_listener();
    let body = subscription_profiles("Original");
    let mut backend = TestBackend::start(directory.options());
    let add = backend.send(ClientCommand::AddSubscription {
        title: "User-Agent fixture".into(),
        url: format!("http://{}{SUBSCRIPTION_PATH}", server.local_addr().unwrap()),
    });
    serve_user_agent_response(&server, DEFAULT, None, ETAG, &body);
    let added = backend.wait(Some(add), "default subscription User-Agent");
    assert_eq!(added.last_error, None);
    let uuid = added.subscriptions[0].subscription_uuid.clone();
    backend.run(ClientCommand::SetSubscriptionUserAgent {
        value: Some(Secret::new(CUSTOM)),
    });
    backend.handle.shutdown().unwrap();

    let backend = TestBackend::start(directory.options());
    let snapshot = backend.handle.snapshot();
    assert!(
        snapshot
            .subscription_user_agent
            .as_ref()
            .map(Secret::expose)
            == Some(CUSTOM)
    );
    assert!(!format!("{snapshot:?}").contains(CUSTOM));
    let refresh = || ClientCommand::RefreshSubscription {
        subscription_uuid: uuid.clone(),
    };
    let request = backend.send(refresh());
    serve_user_agent_response(&server, CUSTOM, None, ETAG, &body);
    assert_eq!(
        backend.wait(Some(request), "custom User-Agent").last_error,
        None
    );

    let invalid = backend
        .handle
        .send(ClientCommand::SetSubscriptionUserAgent {
            value: Some(Secret::new("synthetic\r\nX-Forbidden: value")),
        });
    assert_eq!(
        invalid,
        Err(SubmitError::Rejected(ClientError::InvalidInput))
    );
    let request = backend.send(refresh());
    serve_user_agent_response(&server, CUSTOM, Some(ETAG), ETAG, &body);
    assert_eq!(
        backend
            .wait(Some(request), "unchanged User-Agent")
            .last_error,
        None
    );

    backend.run(ClientCommand::SetSubscriptionUserAgent { value: None });
    let request = backend.send(refresh());
    serve_user_agent_response(&server, DEFAULT, None, ETAG, &body);
    assert_eq!(
        backend
            .wait(Some(request), "restored default User-Agent")
            .last_error,
        None
    );
}

fn serve_user_agent_response(
    server: &TcpListener,
    user_agent: &str,
    expected_version: Option<&str>,
    version: &str,
    body: &str,
) {
    let mut stream = accept(server);
    let (line, headers) = read_http_request(&mut stream);
    assert_eq!(line, format!("GET {SUBSCRIPTION_PATH} HTTP/1.1\r\n"));
    assert!(fixture_headers(&headers, "user-agent").eq([user_agent]));
    assert!(fixture_headers(&headers, "if-none-match").eq(expected_version));
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nConnection: close\r\n\
         ETag: {version}\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
}

fn fixture_headers<'a>(headers: &'a [String], key: &str) -> impl Iterator<Item = &'a str> {
    headers.iter().filter_map(move |header| {
        let (name, value) = header.split_once(':')?;
        name.eq_ignore_ascii_case(key).then_some(value.trim())
    })
}

#[test]
fn conditional_refresh_uses_the_sealed_version_and_keeps_profiles_on_304() {
    const ETAG: &str = r#"W/"fixture-version-1""#;
    let directory = TestDirectory::new();
    let server = loopback_listener();
    let body = subscription_profiles("Original");
    let mut backend = TestBackend::start(directory.options());
    let added = provider_add(
        &backend,
        &server,
        &[("ETag", ETAG), ("hide-settings", "1")],
        &body,
    );
    let uuid = &added.subscriptions[0].subscription_uuid;
    let selected = backend.run(ClientCommand::SelectProfile {
        profile_uuid: added.profiles[1].profile_uuid.clone(),
    });
    backend.handle.shutdown().unwrap();

    let store = SealedCatalogStore::new(directory.path.clone()).unwrap();
    let mut catalog = store.load().unwrap().unwrap();
    catalog.subscriptions[0].provider.last_success_unix = Some(1);
    store.save(&catalog).unwrap();
    drop(store);
    let backend = TestBackend::start(directory.options());
    let command = || ClientCommand::RefreshSubscription {
        subscription_uuid: uuid.clone(),
    };
    let request = backend.send(command());
    serve_versioned_response(
        &server,
        SUBSCRIPTION_PATH,
        "304 Not Modified",
        &[("hide-settings", "0")],
        "",
        Some(ETAG),
    );
    let unchanged = backend.wait(Some(request), "conditional refresh");
    assert_eq!(unchanged.last_error, None);
    assert_eq!(
        unchanged.selected_profile_uuid,
        selected.selected_profile_uuid
    );
    assert!(unchanged.provider_subscriptions[0].raw_config_hidden);
    assert!(
        unchanged.provider_subscriptions[0]
            .last_success_unix
            .unwrap()
            > 1
    );
    let ids = |snapshot: &ClientSnapshot| {
        snapshot
            .profiles
            .iter()
            .map(|profile| profile.profile_uuid.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(ids(&unchanged), ids(&added));

    backend.run(ClientCommand::RemoveProfile {
        profile_uuid: added.profiles[0].profile_uuid.clone(),
    });
    let reloaded = provider_refresh(&backend, &server, uuid, SUBSCRIPTION_PATH, &[], &body);
    assert_eq!(reloaded.last_error, None);
    assert_eq!(reloaded.profiles.len(), 2);
    assert!(!reloaded.provider_subscriptions[0].raw_config_hidden);
    assert!(!reloaded.profiles[0].server.is_empty());
}

#[test]
fn conditional_refresh_drops_the_primary_version_when_fallback_supplies_profiles() {
    const ETAG: &str = r#""primary-version""#;
    let directory = TestDirectory::new();
    let server = loopback_listener();
    let backend = TestBackend::start(directory.options());
    let added = provider_add(
        &backend,
        &server,
        &[("ETag", ETAG)],
        "socks5://127.0.0.1:18001#Primary",
    );
    let uuid = &added.subscriptions[0].subscription_uuid;
    backend.run(ClientCommand::SetSubscriptionFallback {
        subscription_uuid: uuid.clone(),
        url: Some(format!("http://{}/backup", server.local_addr().unwrap())),
    });
    let request = backend.send(ClientCommand::RefreshSubscription {
        subscription_uuid: uuid.clone(),
    });
    serve_versioned_response(
        &server,
        SUBSCRIPTION_PATH,
        "503 Unavailable",
        &[],
        "",
        Some(ETAG),
    );
    serve_provider_response(
        &server,
        "/backup",
        "200 OK",
        &[("ETag", r#""fallback-version""#)],
        "socks5://127.0.0.1:18002#Backup",
    );
    let backup = backend.wait(Some(request), "fallback after conditional request");
    assert_eq!(backup.profiles[0].name, "Backup");
    assert!(backup.provider_subscriptions[0].last_update_used_fallback);

    let recovered = provider_refresh(
        &backend,
        &server,
        uuid,
        SUBSCRIPTION_PATH,
        &[],
        "socks5://127.0.0.1:18001#Recovered",
    );
    assert_eq!(recovered.last_error, None);
    assert_eq!(recovered.profiles[0].name, "Recovered");
    assert!(!recovered.provider_subscriptions[0].last_update_used_fallback);
}

#[test]
fn conditional_refresh_does_not_forward_versions_to_redirects_or_accept_unsolicited_304() {
    const ETAG: &str = r#""original-version""#;
    let directory = TestDirectory::new();
    let server = loopback_listener();
    let backend = TestBackend::start(directory.options());
    let added = provider_add(
        &backend,
        &server,
        &[("ETag", ETAG)],
        "socks5://127.0.0.1:18001#Original",
    );
    let uuid = &added.subscriptions[0].subscription_uuid;
    let command = || ClientCommand::RefreshSubscription {
        subscription_uuid: uuid.clone(),
    };
    let request = backend.send(command());
    serve_versioned_response(
        &server,
        SUBSCRIPTION_PATH,
        "302 Found",
        &[("Location", "/moved")],
        "",
        Some(ETAG),
    );
    serve_provider_response(
        &server,
        "/moved",
        "200 OK",
        &[("ETag", r#""other-version""#)],
        "socks5://127.0.0.1:18002#Moved",
    );
    let moved = backend.wait(Some(request), "same-origin redirect without validator");
    assert_eq!(moved.profiles[0].name, "Moved");

    let request = backend.send(command());
    serve_provider_response(&server, SUBSCRIPTION_PATH, "304 Not Modified", &[], "");
    let declined = backend.wait(Some(request), "reject unsolicited 304");
    assert_subscription_http(declined.last_error, 304, "Not Modified");
    assert_eq!(declined.profiles[0].name, "Moved");
}

#[test]
fn scheduled_refresh_keeps_last_good_and_defers_platform_job_after_restart() {
    let directory = TestDirectory::new();
    let server = loopback_listener();
    let mut seed = TestBackend::start(directory.options());
    provider_add(
        &seed,
        &server,
        &[],
        "#profile-update-interval: 1\nsocks5://127.0.0.1:18001#Original",
    );
    seed.handle.shutdown().unwrap();
    let store = SealedCatalogStore::new(directory.path.clone()).unwrap();
    let mut catalog = store.load().unwrap().unwrap();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    catalog.subscriptions[0].provider.last_success_unix = Some(now - 3597);
    let selection = catalog.selected_profile_uuid.clone();
    store.save(&catalog).unwrap();
    drop(store);

    let mut backend = automatic_backend(&directory);
    assert_listener_unused(&server);
    serve_subscription(&server, "503 Service Unavailable", "");
    let failed =
        backend.wait_for_schedule(|snapshot| snapshot.subscriptions[0].last_error.is_some());
    assert_subscription_http(
        failed.subscriptions[0].last_error,
        503,
        "Service Unavailable",
    );
    assert_eq!(failed.last_error, None);
    assert_eq!(failed.completed_request, None);
    assert_eq!(failed.profiles[0].name, "Original");
    assert_eq!(failed.selected_profile_uuid, selection);
    thread::sleep(Duration::from_millis(150));
    assert_listener_unused(&server);
    backend.handle.shutdown().unwrap();

    let store = SealedCatalogStore::new(directory.path.clone()).unwrap();
    let saved = store.load().unwrap().unwrap();
    let subscription = &saved.subscriptions[0];
    let failed_at = subscription.provider.last_automatic_failure_unix.unwrap();
    assert!(failed_at >= now);
    let uuid = subscription.subscription_uuid.clone();
    drop(store);

    let mut restarted = TestBackend::start(ClientCoreOptions {
        subscription_refresh: SubscriptionRefreshMode::Platform,
        ..directory.options()
    });
    let control = restarted.handle.refresh_control();
    let job = control.periodic().unwrap();
    let deferred = restarted.wait_for_schedule(|_| {
        matches!(
            control.status(),
            RefreshStatus::Scheduled { completed: Some(id), next_unix: Some(next) }
                if id == job.id && next == failed_at + 3_600
        )
    });
    assert_eq!(deferred.profiles[0].name, "Original");
    assert_eq!(deferred.selected_profile_uuid, selection);
    assert_listener_unused(&server);

    let recovered = refresh(
        &restarted,
        &server,
        &uuid,
        "200 OK",
        "#profile-update-interval: 1\nsocks5://127.0.0.1:18001#Recovered",
    );
    assert_eq!(recovered.last_error, None);
    assert_eq!(recovered.profiles[0].name, "Recovered");
    restarted.handle.shutdown().unwrap();
    let store = SealedCatalogStore::new(directory.path.clone()).unwrap();
    let saved = store.load().unwrap().unwrap();
    assert_eq!(
        saved.subscriptions[0].provider.last_automatic_failure_unix,
        None
    );
}

#[test]
fn scheduled_launch_refresh_is_scoped_and_restores_the_catalog() {
    let directory = TestDirectory::new();
    let eligible = loopback_listener();
    let other = loopback_listener();
    let mut seed = TestBackend::start(directory.options());
    let headers = [("subscription-auto-update-open-enable", "1")];
    provider_add(
        &seed,
        &eligible,
        &headers,
        "socks5://127.0.0.1:18001#Original",
    );
    provider_add(&seed, &other, &[], "socks5://127.0.0.1:18002#Other");
    seed.handle.shutdown().unwrap();

    let mut manual = TestBackend::start(directory.options());
    assert_listener_unused(&eligible);
    manual.handle.shutdown().unwrap();
    let mut backend = automatic_backend(&directory);
    serve_subscription(&eligible, "200 OK", "socks5://127.0.0.1:18001#Updated");
    let updated = backend.wait_for_schedule(|snapshot| {
        snapshot
            .profiles
            .iter()
            .any(|profile| profile.name == "Updated")
    });
    assert!(
        updated
            .profiles
            .iter()
            .any(|profile| profile.name == "Other")
    );
    assert!(
        updated
            .profiles
            .iter()
            .all(|profile| profile.name != "Original")
    );
    assert_listener_unused(&eligible);
    assert_listener_unused(&other);
    backend.handle.shutdown().unwrap();
    let reopened = TestBackend::start(directory.options());
    assert!(
        reopened
            .handle
            .snapshot()
            .profiles
            .iter()
            .any(|profile| profile.name == "Updated")
    );
}

#[test]
fn scheduled_refresh_yields_to_interactive_commands_and_resumes() {
    let directory = TestDirectory::new();
    let server = loopback_listener();
    let mut seed = TestBackend::start(directory.options());
    provider_add(
        &seed,
        &server,
        &[("subscription-auto-update-open-enable", "1")],
        "socks5://127.0.0.1:18001#Original",
    );
    seed.handle.shutdown().unwrap();

    let backend = automatic_backend(&directory);
    let mut unanswered = accept(&server);
    let _ = http_start_line(&mut unanswered);
    let started = Instant::now();
    let updated = backend.run(ClientCommand::SetLanguage {
        language: client_core::Language::English,
    });
    assert!(started.elapsed() < Duration::from_secs(4));
    assert_eq!(updated.language, client_core::Language::English);
    assert_eq!(updated.profiles[0].name, "Original");
    assert_eq!(updated.subscriptions[0].last_error, None);
    drop(unanswered);

    serve_provider_response(
        &server,
        SUBSCRIPTION_PATH,
        "200 OK",
        &[("subscription-auto-update-open-enable", "1")],
        "socks5://127.0.0.1:18001#Resumed",
    );
    let resumed = backend.wait_for_schedule(|snapshot| snapshot.profiles[0].name == "Resumed");
    assert_eq!(resumed.language, client_core::Language::English);
    assert_eq!(resumed.subscriptions[0].last_error, None);
    assert_listener_unused(&server);
}

#[test]
fn platform_subscription_refresh_runs_due_work_without_launch_rules() {
    let directory = TestDirectory::new();
    let due = loopback_listener();
    let launch = loopback_listener();
    let mut seed = TestBackend::start(directory.options());
    provider_add(
        &seed,
        &due,
        &[("profile-update-interval", "1")],
        "socks5://127.0.0.1:18001#Periodic",
    );
    provider_add(
        &seed,
        &launch,
        &[("subscription-auto-update-open-enable", "1")],
        "socks5://127.0.0.1:18002#Launch",
    );
    seed.handle.shutdown().unwrap();
    let store = SealedCatalogStore::new(directory.path.clone()).unwrap();
    let mut catalog = store.load().unwrap().unwrap();
    catalog.subscriptions[0].provider.last_success_unix = Some(1);
    store.save(&catalog).unwrap();
    drop(store);

    let backend = TestBackend::start(ClientCoreOptions {
        subscription_refresh: SubscriptionRefreshMode::Platform,
        ..directory.options()
    });
    let refresh = backend.handle.refresh_control();
    backend.wait_for_schedule(|_| {
        matches!(
            refresh.status(),
            RefreshStatus::Scheduled {
                next_unix: Some(3601),
                ..
            }
        )
    });
    assert_listener_unused(&due);
    assert_listener_unused(&launch);

    let request = refresh.periodic().unwrap();
    serve_subscription(
        &due,
        "200 OK",
        "#profile-update-interval: 1\nsocks5://127.0.0.1:18001#Updated",
    );
    let updated = backend.wait_for_schedule(|_| matches!(refresh.status(), RefreshStatus::Scheduled { completed: Some(id), next_unix: Some(next) } if id == request.id && next > 3601));
    assert!(
        updated
            .profiles
            .iter()
            .any(|profile| profile.name == "Updated")
    );
    assert_eq!(updated.completed_request, None);
    assert_listener_unused(&due);
    assert_listener_unused(&launch);
}

fn automatic_backend(directory: &TestDirectory) -> TestBackend {
    TestBackend::start(ClientCoreOptions {
        subscription_refresh: SubscriptionRefreshMode::Automatic,
        ..directory.options()
    })
}

#[test]
fn probe_settings_persist_across_restart_and_reject_invalid_updates_atomically() {
    use client_core::{ProbeHttpMethod, ProbeMethod, ProbeSettings, ProbeStrategy};
    let directory = TestDirectory::new();
    let mut backend = TestBackend::start(directory.options());
    let target = loopback_listener();
    let added = backend.run(ClientCommand::ImportText {
        title: "Configurable probe".into(),
        text: format!("socks5://{}#Probe", target.local_addr().unwrap()),
    });
    let profile_uuid = added.selected_profile_uuid.unwrap();
    let measured = backend.run(ClientCommand::ProbeProfile { profile_uuid });
    assert!(!measured.profile_latencies.is_empty());
    let settings = ProbeSettings {
        method: ProbeMethod::Proxy,
        http_method: ProbeHttpMethod::Head,
        url: "https://probe.example.test/status?fixture=1".into(),
        timeout_ms: 1750,
        attempts: 4,
        warmup: true,
        strategy: ProbeStrategy::Median,
    };
    let saved = backend.run(ClientCommand::SetProbeSettings {
        settings: settings.clone(),
    });
    assert_eq!(saved.probe_settings, settings);
    assert!(saved.profile_latencies.is_empty());
    let invalid = ProbeSettings {
        url: "https://host:notaport/status".into(),
        ..settings.clone()
    };
    assert_eq!(
        backend
            .handle
            .send(ClientCommand::SetProbeSettings { settings: invalid }),
        Err(SubmitError::Rejected(ClientError::InvalidUrl))
    );
    assert_eq!(backend.handle.snapshot().probe_settings, settings);
    backend.handle.shutdown().unwrap();
    let restarted = TestBackend::start(directory.options());
    assert_eq!(restarted.handle.snapshot().probe_settings, settings);
}

#[test]
fn tcp_probe_reports_the_catalog_endpoint_without_starting_blazecore() {
    let directory = TestDirectory::new();
    let target = loopback_listener();
    let backend = TestBackend::start(directory.options());
    let imported = backend.run(ClientCommand::ImportText {
        title: "TCP fixture".into(),
        text: format!("socks5://{}#Probe", target.local_addr().unwrap()),
    });
    assert!(imported.profile_probe_available);
    let profile_uuid = imported.selected_profile_uuid.clone().unwrap();
    let probe = || ClientCommand::ProbeProfile {
        profile_uuid: profile_uuid.clone(),
    };
    let reachable = backend.run(probe());
    assert_eq!(reachable.revision, imported.revision + 1);
    assert_eq!(reachable.profiles.len(), imported.profiles.len());
    assert_eq!(reachable.profiles[0].name, imported.profiles[0].name);
    assert_eq!(
        reachable.selected_profile_uuid,
        imported.selected_profile_uuid
    );
    assert!(matches!(
        reachable.profile_latencies.get(&profile_uuid),
        Some(ProfileLatency::Reachable { .. })
    ));
    assert_eq!(reachable.connection, ConnectionStatus::Disconnected);
    drop(target);
    let unreachable = backend.run(probe());
    assert_eq!(
        unreachable.profile_latencies.get(&profile_uuid),
        Some(&ProfileLatency::Unreachable)
    );
    let removed = backend.run(ClientCommand::RemoveProfile { profile_uuid });
    assert!(removed.profile_latencies.is_empty());
}

#[test]
fn unavailable_xhttp_endpoint_supports_tcp_probe_after_refresh_and_restart() {
    let directory = TestDirectory::new();
    let source = loopback_listener();
    let target = loopback_listener();
    let address = target.local_addr().unwrap();
    let body = format!(
        "vless://a5c41b77-fc31-4ab0-a275-1c67aa9aab31@{address}?type=xhttp&extra=present#Unavailable\n\
         vless://invalid@127.0.0.1:0#Invalid"
    );
    let mut backend = TestBackend::start(directory.options());
    let added = provider_add(&backend, &source, &[], &body);
    assert_eq!(added.last_error, None);
    assert!(added.profiles.is_empty());
    assert!(added.selected_profile_uuid.is_none());
    let uuid = added.subscriptions[0].subscription_uuid.clone();
    let rows = &added.unavailable_profiles[&uuid];
    let probe_id = rows[0].probe_id.clone().expect("probeable XHTTP endpoint");
    assert!(rows[0].probe_endpoint().is_none());
    assert!(rows[1].probe_id.is_none());
    assert_listener_unused(&target);
    let single = backend.run(ClientCommand::ProbeProfile {
        profile_uuid: probe_id.clone(),
    });
    assert!(matches!(
        single.profile_latencies.get(&probe_id),
        Some(ProfileLatency::Reachable { .. })
    ));
    drop(accept(&target));

    let refreshed = provider_refresh(&backend, &source, &uuid, SUBSCRIPTION_PATH, &[], &body);
    assert_eq!(
        refreshed.unavailable_profiles[&uuid][0].probe_id.as_ref(),
        Some(&probe_id)
    );
    backend.handle.shutdown().unwrap();

    let reopened = TestBackend::start(directory.options());
    let group = reopened.run(ClientCommand::ProbeProfiles {
        subscription_uuid: Some(uuid.clone()),
    });
    assert_eq!(
        group.unavailable_profiles[&uuid][0].probe_id.as_ref(),
        Some(&probe_id)
    );
    assert_eq!(group.profile_latencies.len(), 1);
    assert!(matches!(
        group.profile_latencies.get(&probe_id),
        Some(ProfileLatency::Reachable { .. })
    ));
    assert_eq!(group.connection, ConnectionStatus::Disconnected);
    drop(accept(&target));
}

#[test]
fn tcp_probe_batch_limits_subscription_scope_and_reports_each_endpoint() {
    let directory = TestDirectory::new();
    let provider = loopback_listener();
    let reachable = loopback_listener();
    let closed = loopback_listener();
    let closed_addr = closed.local_addr().unwrap();
    let backend = TestBackend::start(directory.options());
    let added = provider_add(
        &backend,
        &provider,
        &[],
        &format!(
            "socks5://{}#Subscribed\nsocks5://{closed_addr}#Closed",
            reachable.local_addr().unwrap()
        ),
    );
    assert_eq!(added.last_error, None);
    let subscription_uuid = added.subscriptions[0].subscription_uuid.clone();
    let subscribed = &added.profiles[0].profile_uuid;
    let unavailable = &added.profiles[1].profile_uuid;
    let manual = loopback_listener();
    backend.run(ClientCommand::ImportText {
        title: "Manual".into(),
        text: format!("socks5://{}#Manual", manual.local_addr().unwrap()),
    });
    drop(closed);
    let scoped = backend.run(ClientCommand::ProbeProfiles {
        subscription_uuid: Some(subscription_uuid),
    });
    assert_eq!(scoped.profile_latencies.len(), 2);
    assert!(matches!(
        scoped.profile_latencies.get(subscribed),
        Some(ProfileLatency::Reachable { .. })
    ));
    assert_eq!(
        scoped.profile_latencies.get(unavailable),
        Some(&ProfileLatency::Unreachable)
    );
    assert_listener_unused(&manual);

    let all = backend.run(ClientCommand::ProbeProfiles {
        subscription_uuid: None,
    });
    assert_eq!(all.profile_latencies.len(), 3);
    let manual_uuid = &all
        .profiles
        .iter()
        .find(|profile| profile.subscription_uuid.is_none())
        .unwrap()
        .profile_uuid;
    assert!(matches!(
        all.profile_latencies.get(manual_uuid),
        Some(ProfileLatency::Reachable { .. })
    ));
    assert_eq!(all.connection, ConnectionStatus::Disconnected);
}

#[test]
fn tunnel_reports_its_mode_and_clears_it_on_disconnect() {
    let directory = TestDirectory::new();
    let backend = tunnel_backend(&directory, RunningConnection::Tun);
    let connected = backend.run(ClientCommand::Connect {
        mode: ConnectionMode::Tun,
    });
    assert_eq!(connected.connection, ConnectionStatus::Connected);
    assert_eq!(connected.active_mode, Some(ConnectionMode::Tun));
    assert_eq!(
        connected.active_profile_uuid,
        connected.selected_profile_uuid
    );
    assert!(connected.local_proxy.is_none() && connected.http_proxy.is_none());
    assert!(!connected.profile_probe_available);
    let probe = backend.send(ClientCommand::ProbeProfile {
        profile_uuid: connected.selected_profile_uuid.unwrap(),
    });
    let rejected = backend.wait(Some(probe), "reject TCP probe while TUN is active");
    assert_eq!(rejected.last_error, Some(ClientError::UnsupportedMode));
    assert!(rejected.profile_latencies.is_empty());
    let batch = backend.send(ClientCommand::ProbeProfiles {
        subscription_uuid: None,
    });
    let rejected = backend.wait(Some(batch), "reject TCP probe batch while TUN is active");
    assert_eq!(rejected.last_error, Some(ClientError::UnsupportedMode));
    assert!(rejected.profile_latencies.is_empty());
    let disconnected = backend.run(ClientCommand::Disconnect);
    assert!(disconnected.profile_probe_available);
    assert_disconnected(&disconnected);
}

#[test]
fn explicit_tcp_probe_remains_available_during_a_tunnel_without_starting_an_engine() {
    let directory = TestDirectory::new();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let backend =
        tunnel_backend_with_probe(&directory, RunningConnection::Tun, false, calls.clone());
    let target = loopback_listener();
    let imported = backend.run(ClientCommand::ImportText {
        title: "TCP endpoint".into(),
        text: format!("socks5://{}#Endpoint", target.local_addr().unwrap()),
    });
    let other = imported.profiles.last().unwrap().profile_uuid.clone();
    backend.run(ClientCommand::SetProbeSettings {
        settings: client_core::ProbeSettings {
            method: client_core::ProbeMethod::Tcp,
            warmup: false,
            ..Default::default()
        },
    });
    let connected = backend.run(ClientCommand::Connect {
        mode: ConnectionMode::Tun,
    });
    assert!(connected.profile_probe_available);
    let checked = backend.run(ClientCommand::ProbeProfile {
        profile_uuid: other.clone(),
    });
    assert!(matches!(
        checked.profile_latencies.get(&other),
        Some(ProfileLatency::Reachable { .. })
    ));
    assert_eq!(checked.connection, ConnectionStatus::Connected);
    assert!(calls.lock().unwrap().is_empty());
    drop(accept(&target));
    assert_disconnected(&backend.run(ClientCommand::Disconnect));
}

#[test]
fn active_tunnel_probe_uses_the_current_engine_even_for_a_tcp_profile() {
    let directory = TestDirectory::new();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let backend =
        tunnel_backend_with_probe(&directory, RunningConnection::Tun, true, calls.clone());
    let connected = backend.run(ClientCommand::Connect {
        mode: ConnectionMode::Tun,
    });
    assert!(connected.profile_probe_available);
    let active = connected.active_profile_uuid.clone().unwrap();
    let measured = backend.run(ClientCommand::ProbeProfile {
        profile_uuid: active.clone(),
    });
    assert_eq!(
        measured.profile_latencies.get(&active),
        Some(&ProfileLatency::Reachable { milliseconds: 17 })
    );
    assert_eq!(
        calls.lock().unwrap().as_slice(),
        std::slice::from_ref(&active)
    );
    assert_eq!(measured.active_profile_uuid.as_ref(), Some(&active));
    assert_eq!(measured.active_mode, Some(ConnectionMode::Tun));
    assert_eq!(measured.connection, ConnectionStatus::Connected);
    assert!(!measured.cleanup_required);
    assert_disconnected(&backend.run(ClientCommand::Disconnect));
}

#[test]
fn active_tunnel_probe_batch_rejects_other_profiles_without_tcp_connections() {
    let directory = TestDirectory::new();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let backend =
        tunnel_backend_with_probe(&directory, RunningConnection::Tun, true, calls.clone());
    let target = loopback_listener();
    let imported = backend.run(ClientCommand::ImportText {
        title: "Alternate".into(),
        text: format!("socks5://{}#Alternate", target.local_addr().unwrap()),
    });
    let other = imported.profiles.last().unwrap().profile_uuid.clone();
    let connected = backend.run(ClientCommand::Connect {
        mode: ConnectionMode::Tun,
    });
    let active = connected.active_profile_uuid.clone().unwrap();
    assert_ne!(active, other);
    let request = backend.send(ClientCommand::ProbeProfile {
        profile_uuid: other.clone(),
    });
    let rejected = backend.wait(Some(request), "refuse another profile while TUN is active");
    assert_eq!(rejected.last_error, Some(ClientError::DisconnectFirst));
    assert!(calls.lock().unwrap().is_empty());
    assert!(rejected.profile_latencies.is_empty());
    let measured = backend.run(ClientCommand::ProbeProfiles {
        subscription_uuid: None,
    });
    assert_eq!(
        measured.profile_latencies.get(&active),
        Some(&ProfileLatency::Reachable { milliseconds: 17 })
    );
    assert_eq!(
        measured.profile_latencies.get(&other),
        Some(&ProfileLatency::Failed(ClientError::DisconnectFirst))
    );
    assert_eq!(*calls.lock().unwrap(), [active]);
    assert_eq!(measured.connection, ConnectionStatus::Connected);
    assert_listener_unused(&target);
    assert_disconnected(&backend.run(ClientCommand::Disconnect));
}

#[test]
fn tunnel_refuses_readiness_from_the_wrong_mode() {
    let directory = TestDirectory::new();
    let backend = tunnel_backend(
        &directory,
        RunningConnection::LocalProxy(SocketAddr::from((Ipv4Addr::LOCALHOST, 1080))),
    );
    let request = backend.send(ClientCommand::Connect {
        mode: ConnectionMode::Tun,
    });
    let failed = backend.wait(Some(request), "connect mismatched mode");
    assert_eq!(failed.connection, ConnectionStatus::Failed);
    assert_eq!(failed.last_error, Some(ClientError::EngineReadiness));
    assert!(failed.cleanup_required);
    assert!(failed.local_proxy.is_none() && failed.http_proxy.is_none());
    assert_disconnected(&backend.run(ClientCommand::Disconnect));
}

#[test]
fn routing_saved_revision_waits_for_reconnect_and_blocks_removal_while_referenced() {
    let directory = TestDirectory::new();
    let backend = routing_backend(&directory, RoutingEngineFixture::default());
    let (uuid, connected) = connect_fixture_routing(&backend);
    let active = connected.routing.active.unwrap();
    let saved = change_fixture_dns(&backend, &uuid, DnsPolicy::System);
    assert_eq!(saved.routing.active, Some(active));
    assert_eq!(saved.routing.saved.unwrap().revision, 2);
    let apply = saved
        .routing
        .pending_reconnect
        .expect("saved revision pending reconnect");

    assert_routing_removal_blocked(&backend, &uuid);
    let reconnected = backend.run(ClientCommand::ReconnectRouting { pending: apply });
    assert_eq!(reconnected.routing.active, reconnected.routing.saved);
    backend.run(assign_global_routing(None));
    assert_routing_removal_blocked(&backend, &uuid);
    backend.run(ClientCommand::Disconnect);
    let removed = backend.run(ClientCommand::RemoveRoutingProfile {
        profile_uuid: uuid.clone(),
    });
    assert!(removed.routing.catalog.lookup(&uuid).is_none());
}

#[test]
fn routing_title_and_unused_policy_do_not_require_reconnect() {
    let directory = TestDirectory::new();
    let backend = routing_backend(&directory, RoutingEngineFixture::default());
    let (uuid, connected) = connect_fixture_routing(&backend);
    assert!(connected.routing.pending_reconnect.is_none());
    save_fixture_routing(&backend, "Unused policy");
    assert!(
        backend
            .handle
            .snapshot()
            .routing
            .pending_reconnect
            .is_none()
    );

    let renamed = backend.run(ClientCommand::SaveRoutingProfile {
        uuid: Some(uuid.clone()),
        profile: RoutingInput {
            title: "New title".into(),
            ..RoutingInput::default()
        },
    });
    let policy = renamed.routing.catalog.lookup(&uuid).unwrap();
    assert_eq!(policy.input.title, "New title");
    assert_eq!(policy.revision, 1);
    assert_eq!(renamed.routing.saved, connected.routing.active);
    assert!(renamed.routing.pending_reconnect.is_none());
}

#[test]
fn routing_restoring_active_rules_cancels_pending_reconnect() {
    let directory = TestDirectory::new();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let backend = routing_backend(
        &directory,
        RoutingEngineFixture {
            calls: calls.clone(),
            ..RoutingEngineFixture::default()
        },
    );
    let (uuid, connected) = connect_fixture_routing(&backend);
    let active = connected.routing.active.unwrap();
    let change_rules = |rules| ClientCommand::SaveRoutingProfile {
        uuid: Some(uuid.clone()),
        profile: RoutingInput {
            title: "Policy".into(),
            rules,
            ..RoutingInput::default()
        },
    };
    let changed = backend.run(change_rules(vec![RouteRule {
        target: RouteTarget::Domain("blocked.invalid".into()),
        action: RouteAction::Block,
    }]));
    assert!(changed.routing.pending_reconnect.is_some());
    let restored = backend.run(change_rules(Vec::new()));
    assert!(restored.routing.pending_reconnect.is_none());
    assert_eq!(restored.routing.active, Some(active));
    assert_eq!(calls.lock().unwrap().len(), 1);
}

#[test]
fn routing_reconnect_rejects_stale_token_and_applies_latest_to_active_server() {
    let directory = TestDirectory::new();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let backend = routing_backend(
        &directory,
        RoutingEngineFixture {
            calls: calls.clone(),
            ..RoutingEngineFixture::default()
        },
    );
    let (uuid, connected) = connect_fixture_routing(&backend);
    assert!(connected.routing.local_proxy_available);
    let active_server = connected.selected_profile_uuid.unwrap();
    let other_server = connected
        .profiles
        .iter()
        .find(|profile| profile.profile_uuid != active_server)
        .unwrap()
        .profile_uuid
        .clone();
    assert_eq!(
        connected.active_profile_uuid.as_deref(),
        Some(active_server.as_str())
    );
    let active = connected.routing.active.unwrap();
    assert_eq!(active.revision, 1);

    let change_server = backend.send(ClientCommand::SelectProfile {
        profile_uuid: other_server,
    });
    let rejected = backend.wait(Some(change_server), "select another server while connected");
    assert_eq!(rejected.last_error, Some(ClientError::DisconnectFirst));

    let pending = change_fixture_dns(&backend, &uuid, DnsPolicy::System);
    let stale = pending
        .routing
        .pending_reconnect
        .expect("first saved revision pending reconnect");
    let newest = change_fixture_dns(
        &backend,
        &uuid,
        DnsPolicy::Tcp {
            endpoint: SocketAddr::from((Ipv4Addr::LOCALHOST, 53)),
        },
    );
    assert_eq!(newest.routing.active, Some(active));
    assert_eq!(newest.routing.saved.unwrap().revision, 3);
    let apply = newest
        .routing
        .pending_reconnect
        .expect("newest saved revision pending reconnect");

    let stale_request = backend.send(ClientCommand::ReconnectRouting { pending: stale });
    let rejected = backend.wait(Some(stale_request), "reject stale routing reconnect");
    assert_eq!(rejected.last_error, Some(ClientError::InvalidInput));
    assert_eq!(rejected.connection, ConnectionStatus::Connected);
    assert_eq!(rejected.routing.active.unwrap().revision, 1);
    assert_eq!(
        calls.lock().unwrap().as_slice(),
        &[RoutingEngineCall::Start {
            profile_uuid: active_server.clone(),
            revision: Some(1),
        }]
    );

    let reconnected = backend.run(ClientCommand::ReconnectRouting { pending: apply });
    assert_latest_routing_active(reconnected, &calls.lock().unwrap(), active_server);
}

fn assert_latest_routing_active(
    reconnected: ClientSnapshot,
    calls: &[RoutingEngineCall],
    active_server: String,
) {
    assert_eq!(reconnected.connection, ConnectionStatus::Connected);
    assert_eq!(
        reconnected.active_profile_uuid.as_deref(),
        Some(active_server.as_str())
    );
    assert_eq!(reconnected.routing.active, reconnected.routing.saved);
    assert!(reconnected.routing.pending_reconnect.is_none());
    assert_eq!(
        calls,
        &[
            RoutingEngineCall::Start {
                profile_uuid: active_server.clone(),
                revision: Some(1)
            },
            RoutingEngineCall::Stop,
            RoutingEngineCall::Start {
                profile_uuid: active_server,
                revision: Some(3)
            },
        ]
    );
}

fn assert_routing_removal_blocked(backend: &TestBackend, uuid: &str) {
    let remove = backend.send(ClientCommand::RemoveRoutingProfile {
        profile_uuid: uuid.into(),
    });
    let blocked = backend.wait(Some(remove), "remove referenced routing profile");
    assert_eq!(
        blocked.last_error,
        Some(ClientError::RoutingProfileAssigned)
    );
}

fn change_fixture_dns(backend: &TestBackend, uuid: &str, policy: DnsPolicy) -> ClientSnapshot {
    backend.run(ClientCommand::SetDnsPolicy {
        profile_uuid: uuid.into(),
        policy,
    })
}

fn connect_fixture_routing(backend: &TestBackend) -> (String, ClientSnapshot) {
    backend.run(ClientCommand::ImportText {
        title: "Routing fixture".into(),
        text: "socks5://127.0.0.1:1080#Test\nsocks5://127.0.0.1:1081#Alternate".into(),
    });
    let uuid = save_fixture_routing(backend, "Policy");
    backend.run(assign_global_routing(Some(uuid.clone())));
    let connected = backend.run(ClientCommand::Connect {
        mode: ConnectionMode::LocalProxy,
    });
    (uuid, connected)
}

#[test]
fn routing_reconnect_reports_start_failure_and_preserves_the_saved_policy() {
    let directory = TestDirectory::new();
    let backend = routing_backend(
        &directory,
        RoutingEngineFixture {
            fail_revision: Some(2),
            ..RoutingEngineFixture::default()
        },
    );
    backend.run(ClientCommand::ImportText {
        title: "Routing fixture".into(),
        text: "socks5://127.0.0.1:1080#Test".into(),
    });
    let uuid = save_fixture_routing(&backend, "Policy");
    backend.run(assign_global_routing(Some(uuid.clone())));
    backend.run(ClientCommand::Connect {
        mode: ConnectionMode::LocalProxy,
    });
    let saved = backend.run(ClientCommand::SetDnsPolicy {
        profile_uuid: uuid,
        policy: DnsPolicy::System,
    });
    let pending = saved
        .routing
        .pending_reconnect
        .expect("saved revision pending reconnect");
    let request = backend.send(ClientCommand::ReconnectRouting { pending });
    let failed = backend.wait(Some(request), "start newest routing policy");
    assert_eq!(failed.last_error, Some(ClientError::EngineStart));
    assert_eq!(failed.connection, ConnectionStatus::Failed);
    assert!(failed.active_profile_uuid.is_none());
    assert!(failed.routing.active.is_none());
    assert!(failed.routing.pending_reconnect.is_none());
    assert_eq!(failed.routing.saved.unwrap().revision, 2);
}

fn routing_backend(directory: &TestDirectory, engine: RoutingEngineFixture) -> TestBackend {
    let handle = ClientHandle::spawn_with_platform(
        directory.path.clone(),
        Box::new(UserSecretProtection),
        Box::new(engine),
        SubscriptionRefreshMode::Manual,
    )
    .expect("spawn backend with routing engine fixture");
    TestBackend::from_handle(handle)
}

fn save_fixture_routing(backend: &TestBackend, title: &str) -> String {
    backend
        .run(ClientCommand::SaveRoutingProfile {
            uuid: None,
            profile: RoutingInput {
                title: title.into(),
                ..RoutingInput::default()
            },
        })
        .routing
        .last_saved_profile_uuid
        .unwrap()
}

fn assign_global_routing(profile_uuid: Option<String>) -> ClientCommand {
    ClientCommand::SetRoutingAssignment {
        subscription_uuid: None,
        assignment: RoutingAssignment {
            enabled: true,
            profile_uuid,
            ignore_provider: false,
        },
    }
}

#[derive(Debug, PartialEq, Eq)]
enum RoutingEngineCall {
    Start {
        profile_uuid: String,
        revision: Option<u64>,
    },
    Stop,
}

#[derive(Default)]
struct RoutingEngineFixture {
    running: bool,
    calls: Arc<Mutex<Vec<RoutingEngineCall>>>,
    fail_revision: Option<u64>,
}

impl ClientEnginePort for RoutingEngineFixture {
    fn capabilities(&self) -> ModeCapabilities {
        ModeCapabilities {
            local_proxy: true,
            ..ModeCapabilities::default()
        }
    }

    fn supports_routing(&self, mode: ConnectionMode) -> bool {
        mode == ConnectionMode::LocalProxy
    }

    fn validate_profile(&self, _: &ProxyProfile) -> Result<(), ClientError> {
        Ok(())
    }

    fn start(
        &mut self,
        mode: ConnectionMode,
        _: &ProxyProfile,
        _: u16,
        cancel: &Cancellation,
    ) -> Result<(), ClientError> {
        assert_eq!(mode, ConnectionMode::LocalProxy);
        cancel.check()?;
        self.running = true;
        Ok(())
    }

    fn start_with_routing(
        &mut self,
        mode: ConnectionMode,
        profile: &ProxyProfile,
        port: u16,
        routing: Option<&RoutingProfile>,
        cancel: &Cancellation,
    ) -> Result<(), ClientError> {
        let revision = routing.map(|routing| routing.revision);
        self.calls.lock().unwrap().push(RoutingEngineCall::Start {
            profile_uuid: profile.profile_uuid.clone(),
            revision,
        });
        if self.fail_revision.is_some() && self.fail_revision == revision {
            return Err(ClientError::EngineStart);
        }
        self.start(mode, profile, port, cancel)
    }

    fn stop(&mut self) -> Result<(), ClientError> {
        self.calls.lock().unwrap().push(RoutingEngineCall::Stop);
        self.running = false;
        Ok(())
    }

    fn poll(&mut self) -> Result<EngineRuntime, ClientError> {
        Ok(if self.running {
            EngineRuntime::Running(RunningConnection::LocalProxy(SocketAddr::from((
                Ipv4Addr::LOCALHOST,
                1080,
            ))))
        } else {
            EngineRuntime::Stopped
        })
    }
}

fn tunnel_backend(directory: &TestDirectory, reported: RunningConnection) -> TestBackend {
    tunnel_backend_with_probe(directory, reported, false, Arc::new(Mutex::new(Vec::new())))
}

fn tunnel_backend_with_probe(
    directory: &TestDirectory,
    reported: RunningConnection,
    active_probe: bool,
    probe_calls: Arc<Mutex<Vec<String>>>,
) -> TestBackend {
    let handle = ClientHandle::spawn_with_platform(
        directory.path.clone(),
        Box::new(UserSecretProtection),
        Box::new(TunnelFixture {
            reported,
            running: false,
            active_probe,
            probe_calls,
        }),
        SubscriptionRefreshMode::Manual,
    )
    .expect("spawn backend with injected engine");
    let backend = TestBackend::from_handle(handle);
    backend.run(ClientCommand::SetConnectionMode {
        mode: ConnectionMode::Tun,
    });
    let imported = backend.run(ClientCommand::ImportText {
        title: "Tunnel fixture".into(),
        text: "socks5://127.0.0.1:1080#Tunnel".into(),
    });
    assert!(imported.selected_profile_uuid.is_some());
    backend
}

struct TunnelFixture {
    reported: RunningConnection,
    running: bool,
    active_probe: bool,
    probe_calls: Arc<Mutex<Vec<String>>>,
}

impl ClientEnginePort for TunnelFixture {
    fn capabilities(&self) -> ModeCapabilities {
        ModeCapabilities {
            tun: true,
            ..ModeCapabilities::default()
        }
    }

    fn validate_profile(&self, _profile: &ProxyProfile) -> Result<(), ClientError> {
        Ok(())
    }

    fn supports_active_tunnel_probe(&self) -> bool {
        self.active_probe
    }

    fn probe_tunnel(
        &self,
        profile: &ProxyProfile,
        active: Option<&ProxyProfile>,
        reuse_active_proxy: bool,
        _settings: &client_core::ProbeSettings,
        cancel: &Cancellation,
    ) -> Result<Duration, ClientError> {
        cancel.check()?;
        assert!(self.running && self.active_probe);
        assert_eq!(self.reported, RunningConnection::Tun);
        assert_eq!(
            active.map(|item| &item.profile_uuid),
            Some(&profile.profile_uuid)
        );
        assert!(!reuse_active_proxy);
        self.probe_calls
            .lock()
            .unwrap()
            .push(profile.profile_uuid.clone());
        Ok(Duration::from_millis(17))
    }

    fn start(
        &mut self,
        mode: ConnectionMode,
        _profile: &ProxyProfile,
        _local_port: u16,
        cancel: &Cancellation,
    ) -> Result<(), ClientError> {
        assert_eq!(mode, ConnectionMode::Tun);
        cancel.check()?;
        self.running = true;
        Ok(())
    }

    fn stop(&mut self) -> Result<(), ClientError> {
        self.running = false;
        Ok(())
    }

    fn poll(&mut self) -> Result<EngineRuntime, ClientError> {
        Ok(if self.running {
            EngineRuntime::Running(self.reported)
        } else {
            EngineRuntime::Stopped
        })
    }
}

#[test]
fn incompatible_profiles_are_listed_after_refresh_and_manual_import_stays_atomic() {
    let directory = TestDirectory::new();
    let server = loopback_listener();
    let backend = TestBackend::start(directory.options());
    let added = provider_add(&backend, &server, &[], &subscription_profiles("Spare"));
    let subscription_uuid = &added.subscriptions[0].subscription_uuid;
    let profile_uuid = &added.profiles[1].profile_uuid;
    backend.run(ClientCommand::SelectProfile {
        profile_uuid: profile_uuid.clone(),
    });

    let body = format!(
        "{}{ENGINE_INCOMPATIBLE_XHTTP}",
        subscription_profiles("Refreshed")
    );
    let import = backend.send(ClientCommand::ImportText {
        title: "Rejected import".into(),
        text: body.clone(),
    });
    let rejected = backend.wait(Some(import), "reject incompatible import");
    assert_eq!(rejected.last_error, Some(ClientError::UnsupportedFeature));
    assert_catalog(&rejected, profile_uuid, "Spare");

    let refreshed = refresh(&backend, &server, subscription_uuid, "200 OK", &body);
    assert_eq!(refreshed.last_error, None);
    assert_eq!(refreshed.subscriptions[0].last_error, None);
    assert_eq!(refreshed.subscriptions[0].profile_count, 3);
    assert_catalog(&refreshed, profile_uuid, "Refreshed");
    let unavailable = &refreshed.unavailable_profiles[subscription_uuid];
    assert_eq!(unavailable.len(), 1);
    assert_eq!(unavailable[0].name, "Engine-incompatible");
    assert_eq!(unavailable[0].reason, ProfileUnavailableReason::Unsupported);
}

fn unavailable_xray_subscription() -> String {
    let template = r#"{
        "remarks": "Xray fixture",
        "dns": { "servers": ["localhost"] },
        "routing": { "domainStrategy": "AsIs", "rules": [] },
        "outbounds": [
            {
                "protocol": "vless",
                "tag": "proxy",
                "settings": { "vnext": [{
                    "address": "localhost", "port": 443,
                    "users": [{ "id": "a5c41b77-fc31-4ab0-a275-1c67aa9aab31", "encryption": "none" }]
                }] },
                "streamSettings": {
                    "network": "xhttp",
                    "security": "tls",
                    "xhttpSettings": { "mode": "packet-up", "path": "/fixture", "extra": { "seqPlacement": "query" } },
                    "tlsSettings": { "fingerprint": "chrome" }
                }
            },
            { "protocol": "freedom", "tag": "direct" },
            { "protocol": "blackhole", "tag": "block" }
        ]
    }"#;
    let records = (1..=6)
        .map(|number| template.replace("Xray fixture", &format!("Xray fixture {number}")))
        .collect::<Vec<_>>()
        .join(",");
    format!("[{records}]")
}

fn assert_unavailable_xray_profiles(snapshot: &ClientSnapshot, uuid: &str) {
    assert_eq!(snapshot.subscriptions[0].profile_count, 6);
    assert!(snapshot.profiles.is_empty());
    assert!(snapshot.selected_profile_uuid.is_none());
    let unavailable = &snapshot.unavailable_profiles[uuid];
    assert_eq!(unavailable.len(), 6);
    for (offset, profile) in unavailable.iter().enumerate() {
        assert_eq!(profile.name, format!("Xray fixture {}", offset + 1));
        assert_eq!(profile.reason, ProfileUnavailableReason::Unsupported);
    }
}

#[test]
fn subscription_with_only_unavailable_profiles_is_saved_and_recovers_after_refresh() {
    let directory = TestDirectory::new();
    let provider = loopback_listener();
    let mut backend = TestBackend::start(directory.options());
    let added = provider_add(&backend, &provider, &[], &unavailable_xray_subscription());
    assert_eq!(added.last_error, None);
    let uuid = added.subscriptions[0].subscription_uuid.clone();
    assert_unavailable_xray_profiles(&added, &uuid);
    backend.handle.shutdown().unwrap();
    let reopened = TestBackend::start(directory.options());
    let restored = reopened.wait(None, "restore unavailable subscription profiles");
    assert_eq!(
        restored.unavailable_profiles.get(&uuid),
        added.unavailable_profiles.get(&uuid)
    );
    let rejected = reopened.send(ClientCommand::Connect {
        mode: ConnectionMode::LocalProxy,
    });
    assert_eq!(
        reopened
            .wait(Some(rejected), "cannot connect without a usable profile")
            .last_error,
        Some(ClientError::NoProfileSelected)
    );
    let refreshed = refresh(
        &reopened,
        &provider,
        &uuid,
        "200 OK",
        "socks5://127.0.0.1:12345#Compatible",
    );
    assert_eq!(refreshed.last_error, None);
    assert_eq!(refreshed.subscriptions[0].profile_count, 1);
    assert!(refreshed.unavailable_profiles.is_empty());
    assert_eq!(refreshed.profiles[0].name, "Compatible");
    assert!(refreshed.selected_profile_uuid.is_some());
}

#[test]
fn subscription_url_replacement_preserves_the_saved_address_on_failure() {
    let directory = TestDirectory::new();
    let options = directory.options();
    let server = loopback_listener();
    let authority = server.local_addr().unwrap();
    let original = format!("http://{authority}{SUBSCRIPTION_PATH}");
    let replacement_path = "/subscription?key=replacement-token";
    let replacement = format!("http://{authority}{replacement_path}");
    let profiles = "socks5://127.0.0.1:12345#Fixture";
    let mut backend = TestBackend::start(options.clone());
    let request = backend.send(ClientCommand::AddSubscription {
        title: "Initial title".into(),
        url: original.clone(),
    });
    serve_subscription(&server, "200 OK", profiles);
    let added = backend.wait(Some(request), "add subscription");
    let uuid = added.subscriptions[0].subscription_uuid.clone();
    assert_eq!(added.subscriptions[0].url.expose(), original);
    assert!(!format!("{added:?}").contains(&original));

    let update = || ClientCommand::UpdateSubscription {
        subscription_uuid: uuid.clone(),
        title: "Edited title".into(),
        url: replacement.clone(),
    };
    let request = backend.send(update());
    serve_provider_response(&server, replacement_path, "503 Unavailable", &[], "");
    let failed = backend.wait(Some(request), "failed URL replacement");
    assert_subscription_http(failed.last_error, 503, "Service Unavailable");
    assert_eq!(failed.subscriptions[0].url.expose(), original);
    assert_eq!(failed.subscriptions[0].title, "Initial title");

    let request = backend.send(update());
    serve_provider_response(&server, replacement_path, "200 OK", &[], profiles);
    let saved = backend.wait(Some(request), "save replacement URL");
    assert_eq!(saved.last_error, None);
    assert_eq!(saved.subscriptions[0].title, "Edited title");
    assert_eq!(saved.subscriptions[0].url.expose(), replacement);
    assert!(!format!("{saved:?}").contains(&replacement));
    backend.handle.shutdown().expect("shutdown before reload");

    let mut reopened = TestBackend::start(options);
    assert_eq!(
        reopened.handle.snapshot().subscriptions[0].url.expose(),
        replacement
    );
    reopened.handle.shutdown().expect("shutdown after reload");
}

#[test]
fn subscription_refresh_and_selection_survive_dpapi_reload() {
    let directory = TestDirectory::new();
    let options = directory.options();
    let server = loopback_listener();
    let mut backend = TestBackend::start(options.clone());

    let request = backend.send(ClientCommand::AddSubscription {
        title: "Loopback subscription".into(),
        url: format!("http://{}{SUBSCRIPTION_PATH}", server.local_addr().unwrap()),
    });
    serve_subscription(&server, "200 OK", &subscription_profiles("Spare"));
    let added = backend.wait(Some(request), "add subscription");
    assert_eq!(added.last_error, None);
    assert_eq!(added.subscriptions.len(), 1);
    assert_eq!(added.subscriptions[0].profile_count, 2);
    assert_eq!(added.profiles.len(), 2);
    let subscription_uuid = added.subscriptions[0].subscription_uuid.clone();
    let primary = added
        .profiles
        .iter()
        .find(|profile| profile.name == "Primary")
        .expect("primary profile in fetched catalog");
    let profile_uuid = primary.profile_uuid.clone();
    assert_eq!(
        primary.subscription_uuid.as_deref(),
        Some(subscription_uuid.as_str())
    );

    backend.run(ClientCommand::SelectProfile {
        profile_uuid: profile_uuid.clone(),
    });
    let failed = refresh(&backend, &server, &subscription_uuid, "503 Unavailable", "");
    assert_subscription_http(failed.last_error, 503, "Service Unavailable");
    assert_subscription_http(
        failed.subscriptions[0].last_error,
        503,
        "Service Unavailable",
    );
    assert_catalog(&failed, &profile_uuid, "Spare");

    let updated = refresh(
        &backend,
        &server,
        &subscription_uuid,
        "200 OK",
        &subscription_profiles("Spare-updated"),
    );
    assert_eq!(updated.last_error, None);
    assert_eq!(updated.subscriptions[0].last_error, None);
    assert_catalog(&updated, &profile_uuid, "Spare-updated");
    backend.handle.shutdown().expect("shutdown before reload");

    let sealed = fs::read(directory.path.join("catalog.sealed")).expect("catalog on disk");
    for plaintext in [
        SUBSCRIPTION_PATH.as_bytes(),
        PROFILE_PASSWORD.as_bytes(),
        b"Loopback subscription",
    ] {
        assert!(
            !sealed
                .windows(plaintext.len())
                .any(|bytes| bytes == plaintext)
        );
    }

    let mut reopened = TestBackend::start(options);
    let reloaded = reopened.handle.snapshot();
    assert_eq!(reloaded.subscriptions.len(), 1);
    assert_eq!(
        reloaded.subscriptions[0].subscription_uuid,
        subscription_uuid
    );
    assert_catalog(&reloaded, &profile_uuid, "Spare-updated");

    let refreshed = refresh(
        &reopened,
        &server,
        &subscription_uuid,
        "200 OK",
        &subscription_profiles("Spare-after-reload"),
    );
    assert_eq!(refreshed.last_error, None);
    assert_catalog(&refreshed, &profile_uuid, "Spare-after-reload");
    reopened.handle.shutdown().expect("shutdown after reload");
}

#[test]
#[ignore = "requires BLAZER_TEST_BLAZECORE_EXE pointing to the SHA-256-pinned BlazeCore binary"]
fn trusted_blazecore_proxies_loopback_and_releases_resources() {
    let source = std::env::var_os("BLAZER_TEST_BLAZECORE_EXE")
        .expect("set BLAZER_TEST_BLAZECORE_EXE to the approved local BlazeCore binary");
    let directory = TestDirectory::new();
    let options = ClientCoreOptions {
        engine_path: fs::canonicalize(source).expect("local BlazeCore binary"),
        ..directory.options()
    };

    let upstream = loopback_listener();
    let echo = loopback_listener();
    let target = echo.local_addr().unwrap();
    let (mut backend, profile_uuid) =
        restart_with_refreshed_profile(options, upstream.local_addr().unwrap());

    let endpoint = connect_proxy(&backend, &profile_uuid);
    for verify_client in [
        assert_socks_echo as fn(SocketAddr, SocketAddr),
        assert_http_connect_echo,
    ] {
        thread::scope(|scope| {
            scope.spawn(|| serve_echo(&echo));
            scope.spawn(|| serve_http_connect(&upstream, target));
            verify_client(endpoint, target);
        });
    }

    let disconnected = backend.run(ClientCommand::Disconnect);
    assert_disconnected(&disconnected);
    assert_proxy_released(&directory.path, endpoint);

    let endpoint = connect_proxy(&backend, &profile_uuid);
    backend
        .handle
        .shutdown()
        .expect("shutdown with running BlazeCore");
    assert_disconnected(&backend.handle.snapshot());
    assert_proxy_released(&directory.path, endpoint);
    assert_eq!(
        backend.handle.send(ClientCommand::Disconnect),
        Err(SubmitError::Stopped)
    );
}

fn restart_with_refreshed_profile(
    options: ClientCoreOptions,
    upstream: SocketAddr,
) -> (TestBackend, String) {
    let server = loopback_listener();
    let mut backend = TestBackend::start(options.clone());
    let request = backend.send(ClientCommand::AddSubscription {
        title: "Loopback proxy subscription".into(),
        url: format!("http://{}{SUBSCRIPTION_PATH}", server.local_addr().unwrap()),
    });
    serve_subscription(&server, "200 OK", "socks5://127.0.0.1:19001#Spare");
    let added = backend.wait(Some(request), "add loopback proxy subscription");
    assert_eq!(added.last_error, None);
    let subscription_uuid = &added.subscriptions[0].subscription_uuid;

    let updated = refresh(
        &backend,
        &server,
        subscription_uuid,
        "200 OK",
        &format!(
            "socks5://127.0.0.1:19001#Spare\nhttp://{upstream}#Loopback-Upstream\n{ENGINE_INCOMPATIBLE_XHTTP}"
        ),
    );
    assert_eq!(updated.last_error, None);
    assert_eq!(updated.unavailable_profiles[subscription_uuid].len(), 1);
    let profile_uuid = updated
        .profiles
        .iter()
        .find(|profile| {
            profile.name == "Loopback-Upstream"
                && profile.subscription_uuid.as_deref() == Some(subscription_uuid.as_str())
        })
        .expect("upstream profile from successful subscription refresh")
        .profile_uuid
        .clone();
    backend.run(ClientCommand::SelectProfile {
        profile_uuid: profile_uuid.clone(),
    });
    backend.handle.shutdown().expect("shutdown before reload");

    let reopened = TestBackend::start(options);
    assert_eq!(
        reopened.handle.snapshot().selected_profile_uuid.as_deref(),
        Some(profile_uuid.as_str())
    );
    (reopened, profile_uuid)
}

struct TestDirectory {
    path: PathBuf,
}

impl TestDirectory {
    fn new() -> Self {
        let uuid = vpn_infrastructure::new_profile_uuid();
        let path = std::env::temp_dir().join(format!("blazer-e2e-{uuid}"));
        fs::create_dir(&path).expect("create isolated test directory");
        Self { path }
    }

    fn options(&self) -> ClientCoreOptions {
        ClientCoreOptions {
            data_dir: self.path.clone(),
            engine_path: self.path.join("absent-blazecore.exe"),
            ..ClientCoreOptions::default()
        }
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_file(self.path.join("catalog.sealed"));
        let _ = fs::remove_file(self.path.join("catalog.lock"));
        let _ = fs::remove_file(self.path.join("subscription-device-id"));
        let _ = fs::remove_dir(self.path.join("runtime"));
        let _ = fs::remove_dir(&self.path);
    }
}

struct TestBackend {
    handle: ClientHandle,
    changed: mpsc::Receiver<()>,
}

impl TestBackend {
    fn start(options: ClientCoreOptions) -> Self {
        let handle = ClientHandle::spawn(options).expect("spawn ClientHandle");
        Self::from_handle(handle)
    }

    fn from_handle(handle: ClientHandle) -> Self {
        let (notify, changed) = mpsc::sync_channel(1);
        handle.set_notifier(Arc::new(move || {
            let _ = notify.try_send(());
        }));
        let backend = Self { handle, changed };
        assert_eq!(backend.wait(None, "backend startup").last_error, None);
        backend
    }

    fn send(&self, command: ClientCommand) -> RequestId {
        self.handle.send(command).expect("backend accepted command")
    }

    fn wait(&self, request: Option<RequestId>, operation: &str) -> ClientSnapshot {
        let deadline = Instant::now() + BACKEND_TIMEOUT;
        loop {
            let snapshot = self.handle.snapshot();
            if !snapshot.busy && snapshot.completed_request == request {
                return snapshot;
            }
            self.changed
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .unwrap_or_else(|error| panic!("{operation}: {error}; last snapshot {snapshot:?}"));
        }
    }

    fn run(&self, command: ClientCommand) -> ClientSnapshot {
        let request = self.send(command);
        let snapshot = self.wait(Some(request), "backend command");
        assert_eq!(snapshot.last_error, None, "{snapshot:?}");
        snapshot
    }

    fn wait_for_schedule(&self, predicate: impl Fn(&ClientSnapshot) -> bool) -> ClientSnapshot {
        let deadline = Instant::now() + SOCKET_TIMEOUT;
        loop {
            let snapshot = self.handle.snapshot();
            if predicate(&snapshot) {
                return snapshot;
            }
            self.changed
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .expect("automatic refresh completed");
        }
    }
}

fn subscription_profiles(spare_name: &str) -> String {
    format!(
        "socks5://127.0.0.1:19001#{spare_name}\n\
         socks5://synthetic-user:{PROFILE_PASSWORD}@127.0.0.1:19002#Primary\n"
    )
}

fn refresh(
    backend: &TestBackend,
    server: &TcpListener,
    subscription_uuid: &str,
    status: &str,
    body: &str,
) -> ClientSnapshot {
    let request = backend.send(ClientCommand::RefreshSubscription {
        subscription_uuid: subscription_uuid.into(),
    });
    serve_subscription(server, status, body);
    backend.wait(Some(request), "refresh subscription")
}

fn assert_catalog(snapshot: &ClientSnapshot, profile_uuid: &str, spare_name: &str) {
    assert_eq!(snapshot.profiles.len(), 2);
    assert!(
        snapshot
            .profiles
            .iter()
            .any(|profile| profile.name == spare_name)
    );
    assert_eq!(
        snapshot.selected_profile_uuid.as_deref(),
        Some(profile_uuid)
    );
    let profile = snapshot
        .profiles
        .iter()
        .find(|profile| profile.profile_uuid == profile_uuid)
        .expect("selected profile in catalog");
    assert_eq!(profile.name, "Primary");
}

fn loopback_listener() -> TcpListener {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind loopback");
    listener.set_nonblocking(true).unwrap();
    listener
}

fn accept(listener: &TcpListener) -> TcpStream {
    let deadline = Instant::now() + SOCKET_TIMEOUT;
    loop {
        match listener.accept() {
            Ok((stream, _)) => return blocking_stream(stream),
            Err(error)
                if error.kind() == io::ErrorKind::WouldBlock && Instant::now() < deadline =>
            {
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => panic!("fixture accept on {:?}: {error}", listener.local_addr()),
        }
    }
}

fn blocking_stream(stream: TcpStream) -> TcpStream {
    stream.set_nonblocking(false).unwrap();
    stream.set_read_timeout(Some(SOCKET_TIMEOUT)).unwrap();
    stream.set_write_timeout(Some(SOCKET_TIMEOUT)).unwrap();
    stream
}

fn connect(address: SocketAddr) -> TcpStream {
    blocking_stream(TcpStream::connect_timeout(&address, SOCKET_TIMEOUT).expect("connect loopback"))
}

fn http_start_line(stream: &mut TcpStream) -> String {
    read_http_request(stream).0
}

fn read_http_request(stream: &mut TcpStream) -> (String, Vec<String>) {
    let mut input = BufReader::new(stream).take(4096);
    let mut first = String::new();
    input.read_line(&mut first).expect("HTTP request line");
    let mut header = String::new();
    let mut headers = Vec::new();
    loop {
        header.clear();
        assert_ne!(input.read_line(&mut header).expect("HTTP header"), 0);
        if header == "\r\n" {
            return (first, headers);
        }
        headers.push(header.clone());
    }
}

fn serve_subscription(server: &TcpListener, status: &str, body: &str) {
    let mut stream = accept(server);
    assert_eq!(
        http_start_line(&mut stream),
        format!("GET {SUBSCRIPTION_PATH} HTTP/1.1\r\n")
    );
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\n\
         Connection: close\r\n\r\n{body}",
        body.len()
    )
    .expect("HTTP subscription response");
}

fn connect_proxy(backend: &TestBackend, profile_uuid: &str) -> SocketAddr {
    let snapshot = backend.run(ClientCommand::Connect {
        mode: ConnectionMode::LocalProxy,
    });
    assert_eq!(snapshot.connection, ConnectionStatus::Connected);
    assert_eq!(snapshot.active_mode, Some(ConnectionMode::LocalProxy));
    assert_eq!(snapshot.active_profile_uuid.as_deref(), Some(profile_uuid));
    assert!(snapshot.mode_capabilities.local_http_proxy);
    let local_proxy = snapshot.local_proxy.expect("mixed proxy endpoint");
    assert_eq!(snapshot.http_proxy.as_deref(), Some(local_proxy.as_str()));
    let endpoint: SocketAddr = local_proxy.parse().unwrap();
    assert_eq!(endpoint.ip(), Ipv4Addr::LOCALHOST);
    assert_ne!(endpoint.port(), 0);
    endpoint
}

fn serve_echo(echo: &TcpListener) {
    let mut stream = accept(echo);
    let payload = read_exact::<{ ECHO_PAYLOAD.len() }>(&mut stream);
    assert_eq!(&payload, ECHO_PAYLOAD);
    stream.write_all(&payload).expect("write echo response");
}

fn serve_http_connect(upstream: &TcpListener, target: SocketAddr) {
    let mut tunnel = accept(upstream);
    assert_eq!(
        http_start_line(&mut tunnel),
        format!("CONNECT {target} HTTP/1.1\r\n")
    );
    let mut echo = connect(target);
    tunnel
        .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
        .expect("confirm upstream CONNECT");
    let payload = read_exact::<{ ECHO_PAYLOAD.len() }>(&mut tunnel);
    echo.write_all(&payload).expect("relay payload to echo");
    let reply = read_exact::<{ ECHO_PAYLOAD.len() }>(&mut echo);
    tunnel.write_all(&reply).expect("relay reply to BlazeCore");
}

fn assert_socks_echo(endpoint: SocketAddr, target: SocketAddr) {
    let mut proxy = connect(endpoint);
    proxy.write_all(&[5, 1, 0]).expect("SOCKS5 greeting");
    assert_eq!(read_exact::<2>(&mut proxy), [5, 0]);

    let mut request = vec![5, 1, 0, 1];
    request.extend(Ipv4Addr::LOCALHOST.octets());
    request.extend(target.port().to_be_bytes());
    proxy.write_all(&request).expect("SOCKS5 CONNECT");
    let [version, status, reserved, address_type] = read_exact::<4>(&mut proxy);
    assert_eq!([version, status, reserved], [5, 0, 0]);
    match address_type {
        1 => _ = read_exact::<6>(&mut proxy),
        4 => _ = read_exact::<18>(&mut proxy),
        other => panic!("unexpected SOCKS5 bound address type {other}"),
    }
    assert_echo(&mut proxy);
}

fn assert_http_connect_echo(endpoint: SocketAddr, target: SocketAddr) {
    let mut proxy = connect(endpoint);
    write!(proxy, "CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n\r\n")
        .expect("local HTTP CONNECT");
    let response = http_start_line(&mut proxy);
    assert!(response.starts_with("HTTP/1.1 200 "), "{response:?}");
    assert_echo(&mut proxy);
}

fn assert_echo(proxy: &mut TcpStream) {
    proxy
        .write_all(ECHO_PAYLOAD)
        .expect("send payload via BlazeCore");
    assert_eq!(&read_exact::<{ ECHO_PAYLOAD.len() }>(proxy), ECHO_PAYLOAD);
}

fn read_exact<const N: usize>(stream: &mut TcpStream) -> [u8; N] {
    let mut buffer = [0; N];
    stream
        .read_exact(&mut buffer)
        .expect("read loopback payload");
    buffer
}

fn assert_disconnected(snapshot: &ClientSnapshot) {
    assert_eq!(snapshot.last_error, None);
    assert_eq!(snapshot.connection, ConnectionStatus::Disconnected);
    assert!(snapshot.active_profile_uuid.is_none());
    assert!(snapshot.active_mode.is_none());
    assert!(!snapshot.cleanup_required);
    assert!(snapshot.local_proxy.is_none());
    assert!(snapshot.http_proxy.is_none());
}

fn assert_proxy_released(data_dir: &Path, endpoint: SocketAddr) {
    let _rebound = TcpListener::bind(endpoint).expect("stopped BlazeCore released its bound port");
    let mut runtime = fs::read_dir(data_dir.join("runtime")).expect("isolated runtime directory");
    assert!(
        runtime.next().is_none(),
        "private engine files were left behind"
    );
}

// Provider tests use actual HTTP requests, the backend worker, and the Windows sealed catalog.
#[test]
fn provider_requires_current_consent_for_cross_origin_fallback_and_migration() {
    let directory = TestDirectory::new();
    let options = directory.options();
    let primary = loopback_listener();
    let alternate = loopback_listener();
    let from_origin = format!("http://{}", primary.local_addr().unwrap());
    let target_origin = format!("http://{}", alternate.local_addr().unwrap());
    let fallback = format!("{target_origin}/backup?key=backup-secret");
    let first_migration = format!("{target_origin}/first?key=obsolete-secret");
    let next_migration = format!("{target_origin}/moved?key=approved-secret");
    let mut backend = TestBackend::start(options.clone());

    let added = provider_add(
        &backend,
        &primary,
        &[
            ("fallback-url", &fallback),
            ("new-url", &first_migration),
            ("hide-settings", "true"),
        ],
        "socks5://127.0.0.1:19001#Primary",
    );
    assert_provider_preview(&added, &from_origin, &target_origin);
    let uuid = added.provider_subscriptions[0].subscription_uuid.clone();
    let backup_consent = proposal_nonce(&added, ProviderChangeKind::Fallback);
    let old_consent = proposal_nonce(&added, ProviderChangeKind::Migration);
    assert_listener_unused(&alternate);
    backend.handle.shutdown().unwrap();

    let mut backend = TestBackend::start(options);
    let reloaded = backend.handle.snapshot();
    assert_eq!(
        proposal_nonce(&reloaded, ProviderChangeKind::Fallback),
        backup_consent
    );
    assert_eq!(
        proposal_nonce(&reloaded, ProviderChangeKind::Migration),
        old_consent
    );
    reject_replaced_provider_migration(
        &backend,
        &primary,
        &uuid,
        &old_consent,
        &next_migration,
        &fallback,
    );
    assert_listener_unused(&alternate);

    let accepted = resolve_provider(&backend, &uuid, &backup_consent, true);
    assert_eq!(accepted.last_error, None);
    assert!(accepted.provider_subscriptions[0].has_committed_fallback);
    let refreshed = provider_refresh(
        &backend,
        &primary,
        &uuid,
        SUBSCRIPTION_PATH,
        &[("hide-settings", "true")],
        "socks5://127.0.0.1:19001#Primary",
    );
    assert_eq!(refreshed.last_error, None);
    assert!(refreshed.provider_subscriptions[0].has_committed_fallback);
    assert_listener_unused(&alternate);
    exercise_provider_backup(&backend, &primary, &alternate, &uuid);
    exercise_provider_migration(
        &backend,
        &primary,
        &alternate,
        &uuid,
        &fallback,
        &next_migration,
    );
    backend.handle.shutdown().unwrap();
}

fn assert_provider_preview(added: &ClientSnapshot, from_origin: &str, target_origin: &str) {
    assert_eq!(added.last_error, None);
    let item = &added.provider_subscriptions[0];
    assert!(item.raw_config_hidden);
    assert!(added.profiles[0].server.is_empty());
    assert!(item.recognized_keys.contains(&ProviderKey::HideSettings));
    assert!(!item.has_committed_fallback);
    assert_eq!(item.pending.len(), 2);
    for proposal in &item.pending {
        assert_eq!(proposal.from_origin, from_origin);
        assert_eq!(proposal.target_origin, target_origin);
    }
    let debug = format!("{added:?}");
    for private in ["backup-secret", "obsolete-secret", "synthetic-url-token"] {
        assert!(!debug.contains(private));
    }
}

fn reject_replaced_provider_migration(
    backend: &TestBackend,
    primary: &TcpListener,
    uuid: &str,
    old_consent: &str,
    next_migration: &str,
    fallback: &str,
) {
    let updated = provider_refresh(
        backend,
        primary,
        uuid,
        SUBSCRIPTION_PATH,
        &[
            ("new-url", next_migration),
            ("fallback-url", fallback),
            ("hide-settings", "true"),
            ("providerid", "irrelevant-id-on-refresh"),
        ],
        "socks5://127.0.0.1:19001#Primary",
    );
    assert!(!format!("{updated:?}").contains("irrelevant-id-on-refresh"));
    let next_consent = proposal_nonce(&updated, ProviderChangeKind::Migration);
    assert_ne!(old_consent, next_consent);
    let refused = resolve_provider(backend, uuid, old_consent, true);
    assert_eq!(refused.last_error, Some(ClientError::InvalidInput));
    let rejected = resolve_provider(backend, uuid, &next_consent, false);
    assert_eq!(rejected.last_error, None);
    assert_eq!(rejected.provider_subscriptions[0].pending.len(), 1);
}

#[test]
fn provider_withdrawal_revokes_pending_consent_only_after_complete_primary_response() {
    const ETAG: &str = r#""with-proposals""#;
    let directory = TestDirectory::new();
    let primary = loopback_listener();
    let alternate = loopback_listener();
    let origin = format!("http://{}", alternate.local_addr().unwrap());
    let fallback = format!("{origin}/backup");
    let migration = format!("{origin}/moved");
    let backend = TestBackend::start(directory.options());
    let added = provider_add(
        &backend,
        &primary,
        &[
            ("ETag", ETAG),
            ("fallback-url", &fallback),
            ("new-url", &migration),
        ],
        "socks5://127.0.0.1:19001#Primary",
    );
    let uuid = &added.subscriptions[0].subscription_uuid;
    let command = || ClientCommand::RefreshSubscription {
        subscription_uuid: uuid.clone(),
    };
    let fallback_nonce = proposal_nonce(&added, ProviderChangeKind::Fallback);
    let migration_nonce = proposal_nonce(&added, ProviderChangeKind::Migration);

    let request = backend.send(command());
    serve_versioned_response(
        &primary,
        SUBSCRIPTION_PATH,
        "304 Not Modified",
        &[],
        "",
        Some(ETAG),
    );
    let unchanged = backend.wait(Some(request), "retain pending for unchanged document");
    assert_eq!(
        proposal_nonce(&unchanged, ProviderChangeKind::Fallback),
        fallback_nonce
    );
    assert_eq!(
        proposal_nonce(&unchanged, ProviderChangeKind::Migration),
        migration_nonce
    );

    let request = backend.send(command());
    serve_versioned_response(
        &primary,
        SUBSCRIPTION_PATH,
        "200 OK",
        &[],
        "socks5://127.0.0.1:19001#Fresh",
        Some(ETAG),
    );
    let withdrawn = backend.wait(Some(request), "drop withdrawn proposals");
    assert_eq!(withdrawn.last_error, None);
    assert!(withdrawn.provider_subscriptions[0].pending.is_empty());
    for nonce in [&fallback_nonce, &migration_nonce] {
        let stale = resolve_provider(&backend, uuid, nonce, true);
        assert_eq!(stale.last_error, Some(ClientError::InvalidInput));
    }
    assert_listener_unused(&alternate);
}

fn exercise_provider_migration(
    backend: &TestBackend,
    primary: &TcpListener,
    alternate: &TcpListener,
    uuid: &str,
    fallback: &str,
    next_migration: &str,
) {
    let on_primary = provider_refresh(
        backend,
        primary,
        uuid,
        SUBSCRIPTION_PATH,
        &[
            ("new-url", next_migration),
            ("fallback-url", fallback),
            ("hide-settings", "true"),
        ],
        "socks5://127.0.0.1:19001#Primary",
    );
    assert!(!on_primary.provider_subscriptions[0].last_update_used_fallback);
    assert_eq!(on_primary.provider_subscriptions[0].pending.len(), 1);
    let consent = proposal_nonce(&on_primary, ProviderChangeKind::Migration);
    let approved = resolve_provider(backend, uuid, &consent, true);
    assert_eq!(approved.last_error, None);
    assert!(!approved.provider_subscriptions[0].has_committed_fallback);
    assert!(approved.provider_subscriptions[0].raw_config_hidden);
    assert_listener_unused(alternate);

    let migrated = provider_refresh(
        backend,
        alternate,
        uuid,
        "/moved?key=approved-secret",
        &[],
        "socks5://127.0.0.1:19003#Moved",
    );
    assert_eq!(migrated.last_error, None);
    assert!(!migrated.provider_subscriptions[0].raw_config_hidden);
    assert_eq!(migrated.profiles[0].name, "Moved");
    assert_listener_unused(primary);
}

fn exercise_provider_backup(
    backend: &TestBackend,
    primary: &TcpListener,
    alternate: &TcpListener,
    uuid: &str,
) {
    let request = backend.send(ClientCommand::RefreshSubscription {
        subscription_uuid: uuid.into(),
    });
    serve_provider_response(
        primary,
        SUBSCRIPTION_PATH,
        "302 Found",
        &[("Location", "/this-redirect-must-not-be-followed")],
        "",
    );
    serve_provider_response(
        alternate,
        "/backup?key=backup-secret",
        "200 OK",
        &[
            ("providerid", "unrelated-provider"),
            ("hide-settings", "false"),
            ("new-url", "http://127.0.0.1:19007/hijack"),
        ],
        "socks5://127.0.0.1:19002#Backup",
    );
    let refreshed = backend.wait(Some(request), "fallback");
    assert_eq!(refreshed.last_error, None);
    assert_eq!(
        refreshed.diagnostic_events.last().map(|event| event.kind),
        Some(ClientEventKind::SubscriptionFallbackUsed)
    );
    let provider = &refreshed.provider_subscriptions[0];
    assert!(provider.last_update_used_fallback);
    assert!(provider.raw_config_hidden);
    assert!(provider.pending.is_empty());
    assert_eq!(refreshed.profiles[0].name, "Backup");
    assert!(refreshed.profiles[0].server.is_empty());
}

#[test]
fn provider_uses_fallback_advertised_in_the_subscription_body() {
    let directory = TestDirectory::new();
    let server = loopback_listener();
    let backend = TestBackend::start(directory.options());
    let alternate = format!(
        "http://{}/backup?key=body-fixture",
        server.local_addr().unwrap()
    );
    let added = provider_add(
        &backend,
        &server,
        &[],
        &format!("#fallback-url {alternate}\n#hide-settings: 1\nsocks5://127.0.0.1:19001#Primary"),
    );
    assert_eq!(added.last_error, None);
    let provider = &added.provider_subscriptions[0];
    assert!(provider.has_committed_fallback);
    assert!(provider.raw_config_hidden);
    assert!(provider.pending.is_empty());
    assert!(added.profiles[0].source.is_none());
    let request = backend.send(ClientCommand::UpdateProfile {
        profile_uuid: added.profiles[0].profile_uuid.clone(),
        text: Secret::new("socks5://127.0.0.1:19005#Unapproved"),
    });
    let blocked = backend.wait(Some(request), "reject hidden profile edit");
    assert_eq!(blocked.last_error, Some(ClientError::UnsupportedFeature));
    assert_eq!(blocked.profiles[0].name, "Primary");

    let request = backend.send(ClientCommand::RefreshSubscription {
        subscription_uuid: added.subscriptions[0].subscription_uuid.clone(),
    });
    serve_provider_response(&server, SUBSCRIPTION_PATH, "503 Unavailable", &[], "");
    serve_provider_response(
        &server,
        "/backup?key=body-fixture",
        "200 OK",
        &[],
        "socks5://127.0.0.1:19002#Backup",
    );
    let updated = backend.wait(Some(request), "use fallback from subscription body");
    assert_eq!(updated.last_error, None);
    assert_eq!(updated.profiles[0].name, "Backup");
    assert!(updated.provider_subscriptions[0].last_update_used_fallback);
    assert!(updated.provider_subscriptions[0].raw_config_hidden);
    assert_eq!(updated.subscriptions[0].url, added.subscriptions[0].url);
}

#[test]
fn provider_same_origin_migration_to_backup_preserves_reload_and_refresh() {
    let directory = TestDirectory::new();
    let options = directory.options();
    let server = loopback_listener();
    let fallback = format!("http://{}/promoted", server.local_addr().unwrap());
    let mut backend = TestBackend::start(options.clone());
    let added = provider_add(
        &backend,
        &server,
        &[("fallback-url", &fallback)],
        "socks5://127.0.0.1:19001#Primary",
    );
    assert!(added.provider_subscriptions[0].has_committed_fallback);
    let uuid = &added.subscriptions[0].subscription_uuid;
    let withdrawn = provider_refresh(
        &backend,
        &server,
        uuid,
        SUBSCRIPTION_PATH,
        &[],
        "socks5://127.0.0.1:19001#Primary",
    );
    assert_eq!(withdrawn.last_error, None);
    assert!(!withdrawn.provider_subscriptions[0].has_committed_fallback);
    let promoted = provider_refresh(
        &backend,
        &server,
        uuid,
        SUBSCRIPTION_PATH,
        &[("new-url", &fallback), ("fallback-url", &fallback)],
        "socks5://127.0.0.1:19001#Primary",
    );
    assert_eq!(promoted.last_error, None);
    assert!(!promoted.provider_subscriptions[0].has_committed_fallback);
    backend.handle.shutdown().unwrap();

    let mut reopened = TestBackend::start(options);
    let refreshed = provider_refresh(
        &reopened,
        &server,
        uuid,
        "/promoted",
        &[],
        "socks5://127.0.0.1:19002#Promoted",
    );
    assert_eq!(refreshed.last_error, None);
    assert_eq!(refreshed.profiles[0].name, "Promoted");
    reopened.handle.shutdown().unwrap();
}

#[test]
fn provider_replaces_metadata_and_rejects_oversized_responses() {
    let directory = TestDirectory::new();
    let options = directory.options();
    let server = loopback_listener();
    let mut backend = TestBackend::start(options.clone());
    let old_keys = [
        "profile-title",
        "routing-enable",
        "tun-type",
        "http-auth-mode",
        "socks-auth-user",
    ];
    let new_keys = [
        "socks-auth-password",
        "http-auth-user",
        "http-auth-password",
        "custom-tunnel-config",
        "socks-auth-mode",
    ];
    let first = large_metadata_document(&old_keys, "Before");
    let second = large_metadata_document(&new_keys, "After");
    let added = provider_add(&backend, &server, &[], &first);
    assert_eq!(added.last_error, None);
    let previous_keys = &added.provider_subscriptions[0].recognized_keys;
    assert_eq!(previous_keys.len(), old_keys.len());
    assert!(previous_keys.contains(&ProviderKey::ProfileTitle));
    let uuid = &added.subscriptions[0].subscription_uuid;
    let updated = provider_refresh(&backend, &server, uuid, SUBSCRIPTION_PATH, &[], &second);
    assert_eq!(updated.last_error, None);
    assert_eq!(updated.profiles[0].name, "After");
    let keys = &updated.provider_subscriptions[0].recognized_keys;
    assert_eq!(keys.len(), new_keys.len());
    assert!(keys.contains(&ProviderKey::CustomTunnelConfig));
    assert!(!keys.contains(&ProviderKey::ProfileTitle));

    let oversized = large_metadata_document(&[old_keys, new_keys].concat(), "Too big");
    let retry = provider_refresh(&backend, &server, uuid, SUBSCRIPTION_PATH, &[], &oversized);
    assert_eq!(retry.last_error, Some(ClientError::PayloadTooLarge));
    assert_eq!(retry.profiles[0].name, "After");
    backend.handle.shutdown().unwrap();

    let mut reopened = TestBackend::start(options);
    let reloaded = reopened.handle.snapshot();
    assert_eq!(reloaded.profiles[0].name, "After");
    assert_eq!(&reloaded.provider_subscriptions[0].recognized_keys, keys);
    reopened.handle.shutdown().unwrap();
}

#[test]
fn provider_refresh_keeps_one_uuid_per_reordered_credential_variant() {
    let directory = TestDirectory::new();
    let server = loopback_listener();
    let mut backend = TestBackend::start(directory.options());
    let first = "socks5://user:one@127.0.0.1:19001#Same";
    let second = "socks5://user:two@127.0.0.1:19001#Same";
    let added = provider_add(&backend, &server, &[], &format!("{first}\n{second}"));
    assert_eq!(added.last_error, None);
    let first_uuid = &added.profiles[0].profile_uuid;
    let second_uuid = &added.profiles[1].profile_uuid;
    assert_ne!(first_uuid, second_uuid);
    backend.run(ClientCommand::SelectProfile {
        profile_uuid: second_uuid.clone(),
    });
    let refreshed = provider_refresh(
        &backend,
        &server,
        &added.subscriptions[0].subscription_uuid,
        SUBSCRIPTION_PATH,
        &[],
        &format!("{second}\n{first}"),
    );
    assert_eq!(refreshed.last_error, None);
    assert_eq!(&refreshed.profiles[0].profile_uuid, second_uuid);
    assert_eq!(&refreshed.profiles[1].profile_uuid, first_uuid);
    assert_eq!(
        refreshed.selected_profile_uuid.as_deref(),
        Some(second_uuid.as_str())
    );
    backend.handle.shutdown().unwrap();
}

#[test]
fn provider_explicit_user_backup_is_not_used_for_invalid_success() {
    let directory = TestDirectory::new();
    let primary = loopback_listener();
    let backup = loopback_listener();
    let mut backend = TestBackend::start(directory.options());
    let first = provider_add(
        &backend,
        &primary,
        &[("hide-settings", "1")],
        "socks5://127.0.0.1:19001#Primary",
    );
    let uuid = &first.subscriptions[0].subscription_uuid;
    let added = backend.run(ClientCommand::ImportText {
        title: "Manual".into(),
        text: "socks5://127.0.0.1:19007#Manual".into(),
    });
    assert!(added.profiles[0].server.is_empty());
    assert_eq!(added.profiles[1].server, "127.0.0.1:19007");

    let backup_path = "/manual-backup?key=independent";
    let configured = backend.run(ClientCommand::SetSubscriptionFallback {
        subscription_uuid: uuid.clone(),
        url: Some(format!(
            "http://{}{backup_path}",
            backup.local_addr().unwrap()
        )),
    });
    assert!(configured.provider_subscriptions[0].has_committed_fallback);
    assert!(configured.provider_subscriptions[0].pending.is_empty());
    assert_listener_unused(&backup);

    let ignored_provider_fallback =
        format!("http://{}/provider-backup", primary.local_addr().unwrap());
    let refreshed = provider_refresh(
        &backend,
        &primary,
        uuid,
        SUBSCRIPTION_PATH,
        &[("fallback-url", &ignored_provider_fallback)],
        "socks5://127.0.0.1:19001#Primary",
    );
    assert_eq!(refreshed.last_error, None);
    assert!(refreshed.provider_subscriptions[0].pending.is_empty());
    assert!(!refreshed.provider_subscriptions[0].raw_config_hidden);

    let malformed = provider_refresh(
        &backend,
        &primary,
        uuid,
        SUBSCRIPTION_PATH,
        &[],
        "!invalid!",
    );
    assert_eq!(malformed.last_error, Some(ClientError::InvalidSubscription));
    assert_eq!(
        malformed
            .profiles
            .iter()
            .find(|profile| profile.subscription_uuid.as_deref() == Some(uuid.as_str()))
            .map(|profile| profile.name.as_str()),
        Some("Primary")
    );
    assert_listener_unused(&backup);

    let request = backend.send(ClientCommand::RefreshSubscription {
        subscription_uuid: uuid.clone(),
    });
    serve_provider_response(&primary, SUBSCRIPTION_PATH, "503 Unavailable", &[], "");
    serve_provider_response(
        &backup,
        backup_path,
        "200 OK",
        &[],
        "socks5://127.0.0.1:19001#Recovered",
    );
    let refreshed = backend.wait(Some(request), "explicit user fallback");
    assert_eq!(refreshed.last_error, None);
    assert!(refreshed.provider_subscriptions[0].last_update_used_fallback);
    backend.handle.shutdown().unwrap();
}

#[test]
fn provider_keeps_same_origin_redirects_and_blocks_cross_origin_redirects() {
    let directory = TestDirectory::new();
    let origin = loopback_listener();
    let alternate = loopback_listener();
    let mut backend = TestBackend::start(directory.options());
    let url = format!("http://{}{SUBSCRIPTION_PATH}", origin.local_addr().unwrap());
    let request = backend.send(ClientCommand::AddSubscription {
        title: "Redirect fixture".into(),
        url: url.clone(),
    });
    let target = format!("http://{}/private", alternate.local_addr().unwrap());
    serve_provider_response(
        &origin,
        SUBSCRIPTION_PATH,
        "302 Found",
        &[("Location", &target)],
        "",
    );
    let blocked = backend.wait(Some(request), "cross-origin redirect");
    assert_subscription_http(blocked.last_error, 302, "Found");
    assert!(blocked.subscriptions.is_empty());
    assert_listener_unused(&alternate);

    let request = backend.send(ClientCommand::AddSubscription {
        title: "Redirect fixture".into(),
        url,
    });
    serve_provider_response(
        &origin,
        SUBSCRIPTION_PATH,
        "302 Found",
        &[("Location", "/direct")],
        "",
    );
    serve_provider_response(
        &origin,
        "/direct",
        "200 OK",
        &[],
        "socks5://127.0.0.1:19001#Redirected",
    );
    let followed = backend.wait(Some(request), "same-origin redirect");
    assert_eq!(followed.last_error, None);
    assert_eq!(followed.profiles[0].name, "Redirected");
    backend.handle.shutdown().unwrap();
}

fn large_metadata_document(keys: &[&str], profile: &str) -> String {
    use std::fmt::Write as _;
    let mut body = String::new();
    // The nine non-title fields alone exceed the aggregate 64 KiB budget.
    let filler = "x".repeat(7400);
    for key in keys {
        let value = if *key == "profile-title" {
            profile
        } else {
            &filler
        };
        writeln!(body, "#{key}: {value}").unwrap();
    }
    writeln!(body, "socks5://127.0.0.1:19001#{profile}").unwrap();
    body
}

fn proposal_nonce(snapshot: &ClientSnapshot, kind: client_core::ProviderChangeKind) -> String {
    snapshot.provider_subscriptions[0]
        .pending
        .iter()
        .find(|entry| entry.kind == kind)
        .expect("provider proposal")
        .nonce
        .clone()
}

fn resolve_provider(
    backend: &TestBackend,
    uuid: &str,
    nonce: &str,
    approve: bool,
) -> ClientSnapshot {
    let command = if approve {
        ClientCommand::ApproveProviderChange {
            subscription_uuid: uuid.into(),
            nonce: nonce.into(),
        }
    } else {
        ClientCommand::RejectProviderChange {
            subscription_uuid: uuid.into(),
            nonce: nonce.into(),
        }
    };
    let request = backend.send(command);
    backend.wait(Some(request), "resolve provider proposal")
}

fn provider_add(
    backend: &TestBackend,
    server: &TcpListener,
    headers: &[(&str, &str)],
    body: &str,
) -> ClientSnapshot {
    let request = backend.send(ClientCommand::AddSubscription {
        title: "Loopback provider".into(),
        url: format!("http://{}{SUBSCRIPTION_PATH}", server.local_addr().unwrap()),
    });
    serve_provider_response(server, SUBSCRIPTION_PATH, "200 OK", headers, body);
    backend.wait(Some(request), "add provider subscription")
}

fn provider_refresh(
    backend: &TestBackend,
    server: &TcpListener,
    uuid: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> ClientSnapshot {
    let request = backend.send(ClientCommand::RefreshSubscription {
        subscription_uuid: uuid.into(),
    });
    serve_provider_response(server, path, "200 OK", headers, body);
    backend.wait(Some(request), "refresh provider subscription")
}

fn assert_listener_unused(listener: &TcpListener) {
    assert!(matches!(listener.accept(), Err(error) if error.kind() == io::ErrorKind::WouldBlock));
}

fn serve_provider_response(
    server: &TcpListener,
    path: &str,
    status: &str,
    headers: &[(&str, &str)],
    body: &str,
) {
    serve_versioned_response(server, path, status, headers, body, None);
}

fn serve_versioned_response(
    server: &TcpListener,
    path: &str,
    status: &str,
    headers: &[(&str, &str)],
    body: &str,
    expected_version: Option<&str>,
) -> Option<String> {
    let mut stream = accept(server);
    let (line, request_headers) = read_http_request(&mut stream);
    assert_eq!(line, format!("GET {path} HTTP/1.1\r\n"));
    let versions = request_headers.iter().filter_map(|header| {
        let (name, value) = header.split_once(':')?;
        name.eq_ignore_ascii_case("if-none-match")
            .then_some(value.trim())
    });
    assert!(versions.eq(expected_version));
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: text/plain\r\nConnection: close\r\n"
    )
    .unwrap();
    if status.starts_with("304 ") {
        assert!(body.is_empty());
    } else {
        write!(stream, "Content-Length: {}\r\n", body.len()).unwrap();
    }
    for (name, value) in headers {
        write!(stream, "{name}: {value}\r\n").unwrap();
    }
    write!(stream, "\r\n{body}").unwrap();
    request_headers.iter().find_map(|header| {
        let (name, value) = header.split_once(':')?;
        name.eq_ignore_ascii_case("x-hwid")
            .then(|| value.trim().to_owned())
    })
}
