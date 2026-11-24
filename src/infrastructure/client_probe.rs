//! Measures direct TCP or native Windows ICMP through the current OS network.

use std::time::{Duration, Instant};

use tokio::net::{TcpStream, lookup_host};
use tokio::runtime::Builder;
use tokio::task::JoinSet;
use vpn_application::client::{Cancellation, ClientError, ProfileProbePort};
use vpn_domain::client::{ProbeMethod, ProbeSettings};

const MAX_CONCURRENT_PROBES: usize = vpn_application::client::MAX_CONCURRENT_PROFILE_PROBES;

pub struct TcpProfileProbe;

impl ProfileProbePort for TcpProfileProbe {
    fn prepare_probe(
        &self,
        host: String,
        port: u16,
        settings: ProbeSettings,
    ) -> Result<vpn_application::client::OwnedProbeTask, ClientError> {
        Ok(Box::new(move |cancel| {
            Self.connect_times(&[(&host, port)], &settings, cancel)?
                .into_iter()
                .next()
                .ok_or(ClientError::Network)?
        }))
    }
    fn supports_icmp(&self) -> bool {
        cfg!(windows)
    }

    fn connect_times(
        &self,
        endpoints: &[(&str, u16)],
        settings: &ProbeSettings,
        cancel: &Cancellation,
    ) -> Result<Vec<Result<Duration, ClientError>>, ClientError> {
        cancel.check()?;
        settings.validate()?;
        if settings.method == ProbeMethod::Proxy
            || (settings.method == ProbeMethod::Icmp && !cfg!(windows))
        {
            return Err(ClientError::UnsupportedMode);
        }
        let runtime = Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| ClientError::Network)?;
        let result = runtime.block_on(async {
            tokio::select! {
                biased;
                () = cancel.cancelled() => Err(ClientError::Cancelled),
                result = measure_endpoints(endpoints, settings) => result,
            }
        });
        runtime.shutdown_background();
        result
    }
}

async fn measure_endpoints(
    endpoints: &[(&str, u16)],
    settings: &ProbeSettings,
) -> Result<Vec<Result<Duration, ClientError>>, ClientError> {
    let mut results = vec![Err(ClientError::Network); endpoints.len()];
    let mut pending = endpoints.iter().copied().enumerate();
    let mut inflight = JoinSet::new();
    for (index, (host, port)) in pending.by_ref().take(MAX_CONCURRENT_PROBES) {
        inflight.spawn(measure_endpoint(
            index,
            host.to_owned(),
            port,
            settings.clone(),
        ));
    }
    while let Some(result) = inflight.join_next().await {
        let (index, time) = result.map_err(|_| ClientError::Network)?;
        results[index] = time;
        if let Some((index, (host, port))) = pending.next() {
            inflight.spawn(measure_endpoint(
                index,
                host.to_owned(),
                port,
                settings.clone(),
            ));
        }
    }
    Ok(results)
}

async fn measure_endpoint(
    index: usize,
    host: String,
    port: u16,
    settings: ProbeSettings,
) -> (usize, Result<Duration, ClientError>) {
    let result = endpoint_samples(&host, port, &settings).await;
    (index, result)
}

async fn endpoint_samples(
    host: &str,
    port: u16,
    settings: &ProbeSettings,
) -> Result<Duration, ClientError> {
    let probe = EndpointProbe::new(settings);
    let timeout = Duration::from_millis(u64::from(settings.timeout_ms));
    if settings.warmup && settings.method != ProbeMethod::Icmp {
        probe.sample(host, port, timeout).await?;
    }
    let mut samples = Vec::with_capacity(usize::from(settings.attempts));
    for _ in 0..settings.attempts {
        samples.push(probe.sample(host, port, timeout).await?);
    }
    settings.aggregate(&samples)
}

enum EndpointProbe {
    Tcp,
    #[cfg(windows)]
    Icmp(winping::AsyncPinger),
}

impl EndpointProbe {
    fn new(settings: &ProbeSettings) -> Self {
        #[cfg(windows)]
        if settings.method == ProbeMethod::Icmp {
            let mut pinger = winping::AsyncPinger::new();
            pinger.set_timeout(settings.timeout_ms);
            return Self::Icmp(pinger);
        }
        let _ = settings;
        Self::Tcp
    }

    async fn sample(
        &self,
        host: &str,
        port: u16,
        timeout: Duration,
    ) -> Result<Duration, ClientError> {
        let request = async {
            match self {
                Self::Tcp => tcp_connect_time(host, port).await,
                #[cfg(windows)]
                Self::Icmp(pinger) => icmp_round_trip(pinger, host, port).await,
            }
        };
        tokio::time::timeout(timeout, request)
            .await
            .unwrap_or(Err(ClientError::Timeout))
    }
}

#[cfg(windows)]
async fn icmp_round_trip(
    pinger: &winping::AsyncPinger,
    host: &str,
    port: u16,
) -> Result<Duration, ClientError> {
    let addresses = lookup_host((host, port))
        .await
        .map_err(|_| ClientError::Network)?;
    let mut failure = ClientError::Network;
    for address in addresses {
        match pinger
            .send(address.ip(), winping::Buffer::new())
            .await
            .result
        {
            Ok(milliseconds) => return Ok(Duration::from_millis(u64::from(milliseconds))),
            Err(winping::Error::Timeout) => failure = ClientError::Timeout,
            Err(_) => failure = ClientError::Network,
        }
    }
    Err(failure)
}

async fn tcp_connect_time(host: &str, port: u16) -> Result<Duration, ClientError> {
    let addresses: Vec<_> = lookup_host((host, port))
        .await
        .map_err(|_| ClientError::Network)?
        .collect();
    let start = Instant::now();
    TcpStream::connect(addresses.as_slice())
        .await
        .map_err(|_| ClientError::Network)?;
    Ok(start.elapsed())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;
    use std::net::{Ipv4Addr, TcpListener};
    use std::sync::Arc;
    use vpn_application::client::CancellationSignal;

    fn cancellation() -> Cancellation {
        Cancellation::new(Arc::default(), 1)
    }

    #[test]
    fn tcp_runs_the_requested_attempts_and_excludes_one_warmup() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        listener.set_nonblocking(true).unwrap();
        let settings = ProbeSettings {
            method: ProbeMethod::Tcp,
            attempts: 3,
            warmup: true,
            ..ProbeSettings::default()
        };
        let results = TcpProfileProbe
            .connect_times(&[("127.0.0.1", port)], &settings, &cancellation())
            .unwrap();
        assert!(results[0].is_ok());
        let mut accepted = 0;
        loop {
            match listener.accept() {
                Ok(_) => accepted += 1,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) => panic!("accept failed: {error}"),
            }
        }
        assert_eq!(accepted, 4);
    }

    #[cfg(windows)]
    #[test]
    fn windows_icmp_measures_echo_without_a_tcp_listener() {
        let settings = ProbeSettings {
            method: ProbeMethod::Icmp,
            attempts: 2,
            warmup: true,
            ..ProbeSettings::default()
        };
        let results = TcpProfileProbe
            .connect_times(&[("127.0.0.1", 0)], &settings, &cancellation())
            .unwrap();
        assert!(results[0].is_ok(), "native ICMP failed: {:?}", results[0]);
        assert!(TcpProfileProbe.supports_icmp());
    }

    #[test]
    fn cancellation_stops_before_resolving_or_opening_a_socket() {
        let signal = Arc::new(CancellationSignal::default());
        signal.cancel(1);
        assert_eq!(
            TcpProfileProbe.connect_times(
                &[("unused.invalid", 1)],
                &ProbeSettings::default(),
                &Cancellation::new(signal, 1)
            ),
            Err(ClientError::Cancelled)
        );
    }
}
