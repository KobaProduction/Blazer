use std::net::SocketAddr;
use std::time::{Duration, Instant};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use hyper_util::client::legacy::connect::HttpInfo;
use vpn_application::client::{Cancellation, ClientEnginePort};
use vpn_domain::client::{
    ClientError, ConnectionMode, NativeProxyKind, ProbeHttpMethod, ProbeSettings, ProxyProfile,
    ProxyProtocol,
};
use zeroize::Zeroizing;

use super::{BlazeCoreProcess, RunningConnection};

const MAX_RESPONSE_BYTES: usize = 256 * 1024;

impl BlazeCoreProcess {
    #[cfg(test)]
    pub(super) fn probe_tunnel_at(
        &self,
        profile: &ProxyProfile,
        active: Option<&ProxyProfile>,
        reuse_active_proxy: bool,
        cancel: &Cancellation,
        url: &str,
    ) -> Result<Duration, ClientError> {
        self.probe_tunnel_with_settings(
            profile,
            active,
            reuse_active_proxy,
            &ProbeSettings {
                url: url.to_owned(),
                http_method: ProbeHttpMethod::Get,
                attempts: 1,
                warmup: false,
                ..ProbeSettings::default()
            },
            cancel,
        )
    }

    pub(super) fn probe_tunnel_with_settings(
        &self,
        profile: &ProxyProfile,
        active: Option<&ProxyProfile>,
        reuse_active_proxy: bool,
        settings: &ProbeSettings,
        cancel: &Cancellation,
    ) -> Result<Duration, ClientError> {
        settings.validate()?;
        cancel.check()?;
        if self.connection == Some(RunningConnection::Tun) {
            if active.is_some_and(|active| active.profile_uuid == profile.profile_uuid) {
                return check_request(None, settings, cancel);
            }
            // A different server cannot be measured through the active TUN:
            // starting another engine would test a nested route and can replace
            // an established WireGuard peer's endpoint.
            return Err(ClientError::DisconnectFirst);
        }
        if let Some(active) = active {
            if reuse_active_proxy
                && active.profile_uuid == profile.profile_uuid
                && let Some(address) = self.connection.and_then(RunningConnection::proxy_address)
            {
                return check_request(Some(address), settings, cancel);
            }
            // A second WireGuard handshake for the same identity can replace the active peer's endpoint.
            if same_wireguard_identity(profile, active) {
                return Err(ClientError::DisconnectFirst);
            }
        }
        let mut probe = Self::new(self.spec.clone(), self.data_directory.clone(), 0);
        probe.start(ConnectionMode::LocalProxy, profile, 0, cancel)?;
        let address = probe
            .connection
            .and_then(RunningConnection::proxy_address)
            .ok_or(ClientError::EngineReadiness)?;
        let result = check_request(Some(address), settings, cancel);
        let stopped = probe.stop();
        result.and_then(|elapsed| stopped.map(|()| elapsed))
    }
}

fn same_wireguard_identity(first: &ProxyProfile, second: &ProxyProfile) -> bool {
    let second_keys: Vec<_> = profile_members(second).filter_map(wireguard_key).collect();
    profile_members(first)
        .filter_map(wireguard_key)
        .any(|key| second_keys.contains(&key))
}

fn profile_members(profile: &ProxyProfile) -> impl Iterator<Item = &ProxyProfile> {
    let graph = match &profile.protocol {
        ProxyProtocol::Graph(graph) => Some(graph.as_ref()),
        _ => None,
    };
    std::iter::once(profile).chain(graph.into_iter().flat_map(|graph| {
        graph.outbounds.iter().filter_map(|outbound| {
            if let vpn_domain::client::GraphOutboundKind::Proxy(profile) = &outbound.kind {
                Some(profile.as_ref())
            } else {
                None
            }
        })
    }))
}

fn wireguard_key(profile: &ProxyProfile) -> Option<Zeroizing<[u8; 33]>> {
    let ProxyProtocol::Native(native) = &profile.protocol else {
        return None;
    };
    if !matches!(
        native.kind,
        NativeProxyKind::WireGuard | NativeProxyKind::AmneziaWg
    ) {
        return None;
    }
    let settings = crate::client_normalize::parse_json_value(native.settings.expose()).ok()?;
    let encoded = settings.get("protocol")?.get("private_key")?.as_str()?;
    let mut key = Zeroizing::new([0_u8; 33]);
    if STANDARD.decode_slice(encoded, &mut key[..]).ok()? != 32 {
        return None;
    }
    // X25519 clamps these bits before deriving the peer identity.
    key[0] &= 248;
    key[31] = (key[31] & 127) | 64;
    Some(key)
}

fn check_request(
    proxy: Option<SocketAddr>,
    settings: &ProbeSettings,
    cancel: &Cancellation,
) -> Result<Duration, ClientError> {
    let mut client = reqwest::Client::builder()
        .tls_backend_native()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_millis(u64::from(settings.timeout_ms)));
    if let Some(address) = proxy {
        // The mixed listener's SOCKS transport preserves the origin HTTP/TLS
        // connection across warm-up and measured requests. Resolve the target
        // through the selected engine rather than the host's DNS/proxy policy.
        let proxy = reqwest::Proxy::all(format!("socks5h://{address}"))
            .map_err(|_| ClientError::Network)?;
        client = client.proxy(proxy);
    }
    let client = client.build().map_err(|_| ClientError::Network)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| ClientError::Network)?;
    let result = runtime.block_on(collect_samples(&client, settings, cancel));
    runtime.shutdown_background();
    result
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ConnectionIdentity {
    local: SocketAddr,
    remote: SocketAddr,
}

struct ProbeResponse {
    elapsed: Duration,
    connection: Option<ConnectionIdentity>,
}

async fn collect_samples(
    client: &reqwest::Client,
    settings: &ProbeSettings,
    cancel: &Cancellation,
) -> Result<Duration, ClientError> {
    let warmed = settings.http_warmup_required();
    let mut anchor = if warmed {
        Some(connection_identity(
            measure_request(client, settings, cancel).await?,
        )?)
    } else {
        None
    };
    let mut samples = Vec::with_capacity(usize::from(settings.attempts));
    let mut cold = 0u8;
    while samples.len() < usize::from(settings.attempts) {
        let response = measure_request(client, settings, cancel).await?;
        if warmed {
            let connection = connection_identity_ref(&response)?;
            if Some(connection) != anchor {
                anchor = Some(connection);
                cold += 1;
                if cold > settings.attempts {
                    return Err(ClientError::ProbeConnectionNotReusable);
                }
                continue;
            }
        }
        samples.push(response.elapsed);
    }
    settings.aggregate(&samples)
}

fn connection_identity(response: ProbeResponse) -> Result<ConnectionIdentity, ClientError> {
    connection_identity_ref(&response)
}

fn connection_identity_ref(response: &ProbeResponse) -> Result<ConnectionIdentity, ClientError> {
    response
        .connection
        .ok_or(ClientError::ProbeConnectionNotReusable)
}

async fn measure_request(
    client: &reqwest::Client,
    settings: &ProbeSettings,
    cancel: &Cancellation,
) -> Result<ProbeResponse, ClientError> {
    cancel.check()?;
    let start = Instant::now();
    let result = async {
        let method = match settings.http_method {
            ProbeHttpMethod::Get => reqwest::Method::GET,
            ProbeHttpMethod::Head => reqwest::Method::HEAD,
        };
        let mut response = client
            .request(method, &settings.url)
            .send()
            .await
            .map_err(network_error)?;
        if !response.status().is_success() {
            return Err(ClientError::Network);
        }
        let connection = response
            .extensions()
            .get::<HttpInfo>()
            .map(|info| ConnectionIdentity {
                local: info.local_addr(),
                remote: info.remote_addr(),
            });
        let mut read = 0usize;
        while let Some(chunk) = response.chunk().await.map_err(network_error)? {
            read = read.checked_add(chunk.len()).ok_or(ClientError::Network)?;
            if read > MAX_RESPONSE_BYTES {
                return Err(ClientError::Network);
            }
        }
        Ok(ProbeResponse {
            elapsed: start.elapsed(),
            connection,
        })
    };
    tokio::select! {
        biased;
        () = cancel.cancelled() => Err(ClientError::Cancelled),
        result = result => result,
    }
}

#[cfg(test)]
mod tests;

fn network_error(error: reqwest::Error) -> ClientError {
    if error.is_timeout() {
        ClientError::Timeout
    } else {
        ClientError::Network
    }
}
