use client_core::{
    ClientError, ClientSnapshot, ConnectionMode, DnsPolicy, Language, ProfileLatency,
    ProfileUnavailableReason, RouteAction, RouteTarget, RoutingSelection,
};

use crate::command::Completion;

pub fn help() {
    println!("Usage: blazer --cli <command>");
    println!("  status | profiles | subscriptions | settings | diagnostics");
    println!("  import");
    println!("  profile select|remove|test <uuid>");
    println!("  profiles test | subscription test <uuid>");
    println!("  subscription add <title> | edit|rename <uuid> <title>");
    println!("  subscription refresh|remove|fallback|clear-fallback <uuid>");
    println!("  subscription approve|reject <uuid> <nonce>");
    println!("  language en|ru");
    println!("  mode proxy|system-proxy|tun");
    println!("  local-proxy-port <0-65535>");
    println!("  routing | routing show <profile-uuid>");
    println!("  routing add <title> | routing edit <profile-uuid> <title>");
    println!("  routing copy <profile-uuid> <title>");
    println!("  routing remove <profile-uuid>");
    println!("  routing assign <global|subscription-uuid> <profile-uuid|none> <enabled|disabled>");
    println!("  routing dns <profile-uuid> <automatic|system|tcp|tls|https> [parameters]");
    println!("  proxy | system-proxy | tun");
    println!("Import, subscription add, edit and fallback read their input from stdin until EOF.");
    println!("Connection commands serve the selected profile until Enter or EOF on stdin.");
    println!("Routing add and edit read ordered lines from stdin until EOF.");
    println!("  <proxy|direct|block> <domain|suffix|cidr> <target>");
    println!("  dns <policy> [parameters] (optional, defaults to automatic)");
    println!("DNS parameters");
    println!("  tcp <ip:port>");
    println!("  tls <ip:port> <server-name>");
    println!("  https <ip:port> <server-name> <path>");
}

pub fn status(snapshot: &ClientSnapshot, engine_present: bool) {
    let engine = if engine_present {
        "detected"
    } else {
        "unavailable"
    };
    println!("engine={engine}");
    println!(
        "connect_enabled={}",
        engine_present && snapshot.selected_profile_uuid.is_some()
    );
    println!(
        "local_proxy_supported={}",
        snapshot.mode_capabilities.local_proxy
    );
    println!(
        "local_http_proxy_supported={}",
        snapshot.mode_capabilities.local_http_proxy
    );
    println!(
        "system_proxy_supported={}",
        snapshot.mode_capabilities.system_proxy
    );
    println!("tun_supported={}", snapshot.mode_capabilities.tun);
    let telemetry = if snapshot.telemetry_export_configured {
        "configured"
    } else {
        "not_configured"
    };
    println!("telemetry_export={telemetry}");
}

pub fn profiles(snapshot: &ClientSnapshot) {
    if let Some(uuid) = &snapshot.selected_profile_uuid {
        println!("selected_profile_uuid={uuid}");
    }
    for profile in &snapshot.profiles {
        println!(
            "profile={} protocol={} subscription={} name={:?}",
            profile.profile_uuid,
            profile.protocol,
            profile.subscription_uuid.as_deref().unwrap_or("-"),
            profile.name,
        );
    }
    for subscription in &snapshot.subscriptions {
        let uuid = &subscription.subscription_uuid;
        let Some(profiles) = snapshot.unavailable_profiles.get(uuid) else {
            continue;
        };
        for profile in profiles {
            let reason = match profile.reason {
                ProfileUnavailableReason::Unsupported => "unsupported",
                ProfileUnavailableReason::UnsupportedXhttpUploadMethod => {
                    "unsupported_xhttp_upload_method"
                }
                ProfileUnavailableReason::Invalid => "invalid",
            };
            let name = if profile.name.is_empty() {
                "Unnamed server"
            } else {
                &profile.name
            };
            let protocol = profile.protocol.as_deref().unwrap_or("-");
            let probe = profile
                .probe_id
                .as_ref()
                .map_or_else(String::new, |id| format!(" probe_id={id}"));
            println!(
                "profile_unavailable={reason} protocol={protocol} subscription={uuid} name={name:?}{probe}"
            );
        }
    }
}

pub fn error_message(error: ClientError) -> String {
    if let ClientError::SubscriptionHttp(error) = error {
        let status = error.status();
        return error.detail().map_or_else(
            || format!("error=SubscriptionHttp status={status}"),
            |detail| format!("error=SubscriptionHttp status={status} detail={detail:?}"),
        );
    }
    format!("error={error:?}")
}

pub fn subscriptions(snapshot: &ClientSnapshot) {
    for subscription in &snapshot.subscriptions {
        println!(
            "subscription={} profiles={} title={:?}",
            subscription.subscription_uuid, subscription.profile_count, subscription.title
        );
        if let Some(error) = subscription.last_error {
            println!(
                "subscription_error={} {}",
                subscription.subscription_uuid,
                error_message(error)
            );
        }
    }
    for provider in &snapshot.provider_subscriptions {
        for preview in &provider.pending {
            println!(
                "provider_change={} nonce={} kind={:?} from={:?} to={:?}",
                provider.subscription_uuid,
                preview.nonce,
                preview.kind,
                preview.from_origin,
                preview.target_origin
            );
        }
    }
}

pub fn settings(snapshot: &ClientSnapshot) {
    let language = match snapshot.language {
        Language::Russian => "ru",
        Language::English => "en",
    };
    let mode = match snapshot.selected_mode {
        ConnectionMode::LocalProxy => "proxy",
        ConnectionMode::SystemProxy => "system-proxy",
        ConnectionMode::Tun => "tun",
    };
    println!("language={language}");
    println!("mode={mode}");
    println!("local_proxy_port={}", snapshot.local_proxy_port);
}

pub fn diagnostics(snapshot: &ClientSnapshot) {
    println!(
        "buffered={} dropped={}",
        snapshot.diagnostics.buffered, snapshot.diagnostics.dropped
    );
    for event in &snapshot.diagnostic_events {
        println!("event={} kind={:?}", event.sequence, event.kind);
    }
}

pub fn routing(snapshot: &ClientSnapshot, uuid: Option<&str>) -> Result<(), ClientError> {
    let routing = &snapshot.routing;
    if let Some(uuid) = uuid {
        let profile = routing
            .catalog
            .lookup(uuid)
            .ok_or(ClientError::RoutingProfileMissing)?;
        println!(
            "routing_profile={} revision={} title={:?}",
            profile.uuid, profile.revision, profile.input.title
        );
        dns(&profile.input.dns);
        for rule in &profile.input.rules {
            let action = match rule.action {
                RouteAction::Proxy => "proxy",
                RouteAction::Direct => "direct",
                RouteAction::Block => "block",
            };
            let (kind, value) = match &rule.target {
                RouteTarget::Domain(value) => ("domain", value),
                RouteTarget::DomainSuffix(value) => ("suffix", value),
                RouteTarget::Cidr(value) => ("cidr", value),
            };
            println!("{action} {kind} {value}");
        }
        return Ok(());
    }

    println!(
        "routing_local_proxy_supported={}",
        routing.local_proxy_available
    );
    println!(
        "routing_global={}",
        routing
            .catalog
            .default_profile_uuid
            .as_deref()
            .unwrap_or("none")
    );
    routing_selection("routing_saved", routing.saved.as_ref());
    routing_selection("routing_active", routing.active.as_ref());
    for profile in &routing.catalog.profiles {
        println!(
            "routing_profile={} revision={} title={:?}",
            profile.uuid, profile.revision, profile.input.title
        );
    }
    for (uuid, assignment) in &routing.catalog.assignments {
        println!(
            "routing_subscription={} profile={} enabled={} ignore_provider={}",
            uuid,
            assignment.profile_uuid.as_deref().unwrap_or("none"),
            assignment.enabled,
            assignment.ignore_provider
        );
    }
    Ok(())
}

fn dns(policy: &DnsPolicy) {
    match policy {
        DnsPolicy::Automatic => println!("dns automatic"),
        DnsPolicy::System => println!("dns system"),
        DnsPolicy::Tcp { endpoint } => println!("dns tcp {endpoint}"),
        DnsPolicy::Tls {
            endpoint,
            server_name,
        } => println!("dns tls {endpoint} {server_name}"),
        DnsPolicy::Https {
            endpoint,
            server_name,
            path,
        } => {
            println!("dns https {endpoint} {server_name} {path}");
        }
    }
}

fn routing_selection(label: &str, selection: Option<&RoutingSelection>) {
    match selection {
        Some(selection) => println!(
            "{label}={} revision={}",
            selection.profile_uuid, selection.revision
        ),
        None => println!("{label}=none"),
    }
}

pub fn completion(completion: Completion, snapshot: &ClientSnapshot) -> Result<(), ClientError> {
    match completion {
        Completion::Imported => println!("import=ok"),
        Completion::Updated => println!("command=ok"),
        Completion::RoutingSaved => {
            let routing = &snapshot.routing;
            let profile = routing
                .last_saved_profile_uuid
                .as_deref()
                .and_then(|uuid| routing.catalog.lookup(uuid))
                .ok_or(ClientError::RoutingProfileMissing)?;
            println!(
                "routing_profile_saved={} revision={}",
                profile.uuid, profile.revision
            );
        }
        Completion::Probe(uuid) => {
            let result = snapshot
                .profile_latencies
                .get(&uuid)
                .ok_or(ClientError::ProfileNotFound)?;
            println!("{}", probe_result(result));
        }
        Completion::ProbeProfiles => probe_results(snapshot),
    }
    Ok(())
}

fn probe_results(snapshot: &ClientSnapshot) {
    let available = snapshot
        .profiles
        .iter()
        .map(|profile| &profile.profile_uuid);
    let unavailable = snapshot
        .subscriptions
        .iter()
        .filter_map(|item| snapshot.unavailable_profiles.get(&item.subscription_uuid))
        .flatten()
        .filter_map(|profile| profile.probe_id.as_ref());
    println!("probed={}", snapshot.profile_latencies.len());
    for id in available.chain(unavailable) {
        if let Some(result) = snapshot.profile_latencies.get(id) {
            println!("profile={id} {}", probe_result(result));
        }
    }
}

fn probe_result(result: &ProfileLatency) -> String {
    match result {
        ProfileLatency::Reachable { milliseconds } => {
            format!("probe=reachable milliseconds={milliseconds}")
        }
        ProfileLatency::TimedOut => "probe=timed_out".to_owned(),
        ProfileLatency::Unreachable => "probe=unreachable".to_owned(),
    }
}

pub fn connection(snapshot: &ClientSnapshot, mode: ConnectionMode) -> Result<(), ClientError> {
    if snapshot.active_mode != Some(mode) {
        return Err(ClientError::EngineReadiness);
    }
    if mode == ConnectionMode::Tun {
        println!("tun=connected");
    } else {
        let address = snapshot
            .local_proxy
            .as_deref()
            .ok_or(ClientError::EngineReadiness)?;
        println!("local_proxy={address}");
        if let Some(address) = &snapshot.http_proxy {
            println!("http_proxy={address}");
        }
    }
    if let Some(selection) = &snapshot.routing.active {
        routing_selection("routing_active", Some(selection));
    }
    Ok(())
}
