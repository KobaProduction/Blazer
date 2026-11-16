//! Times a TCP connection to a catalog endpoint through the platform's current network.

use std::time::{Duration, Instant};

use tokio::net::{TcpStream, lookup_host};
use tokio::runtime::Builder;
use tokio::task::JoinSet;
use vpn_application::client::{Cancellation, ClientError, ProfileProbePort};

const MAX_CONCURRENT_PROBES: usize = 24;
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

pub struct TcpProfileProbe;

impl ProfileProbePort for TcpProfileProbe {
    fn connect_times(
        &self,
        endpoints: &[(&str, u16)],
        cancel: &Cancellation,
    ) -> Result<Vec<Result<Duration, ClientError>>, ClientError> {
        cancel.check()?;
        let runtime = Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| ClientError::Network)?;
        let result = runtime.block_on(async {
            tokio::select! {
                biased;
                () = cancel.cancelled() => Err(ClientError::Cancelled),
                result = measure_endpoints(endpoints) => result,
            }
        });
        runtime.shutdown_background();
        result
    }
}

async fn measure_endpoints(
    endpoints: &[(&str, u16)],
) -> Result<Vec<Result<Duration, ClientError>>, ClientError> {
    let mut results = vec![Err(ClientError::Network); endpoints.len()];
    let mut pending = endpoints.iter().copied().enumerate();
    let mut inflight = JoinSet::new();
    for (index, (host, port)) in pending.by_ref().take(MAX_CONCURRENT_PROBES) {
        inflight.spawn(measure_endpoint(index, host.to_owned(), port));
    }
    while let Some(result) = inflight.join_next().await {
        let (index, time) = result.map_err(|_| ClientError::Network)?;
        results[index] = time;
        if let Some((index, (host, port))) = pending.next() {
            inflight.spawn(measure_endpoint(index, host.to_owned(), port));
        }
    }
    Ok(results)
}

async fn measure_endpoint(
    index: usize,
    host: String,
    port: u16,
) -> (usize, Result<Duration, ClientError>) {
    let result = tokio::time::timeout(PROBE_TIMEOUT, tcp_connect_time(&host, port))
        .await
        .unwrap_or(Err(ClientError::Timeout));
    (index, result)
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
