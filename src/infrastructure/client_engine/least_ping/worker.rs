use std::collections::HashMap;
use std::sync::Arc;
use std::sync::mpsc::SyncSender;
use std::time::{Duration, Instant};

use tokio::task::JoinSet;
use vpn_domain::client::{GraphStrategy, ProbeReport, record_graph_probe, select_least_ping};

use super::{Cancellation, ClientError, Prepared, SelectorControl, graph};

struct Probe {
    tag: String,
    client: Option<reqwest::Client>,
}

struct Observer {
    prepared: Prepared,
    control: Arc<dyn SelectorControl>,
    probes: Vec<Probe>,
    reports: Vec<ProbeReport>,
    selected: HashMap<String, String>,
}

pub(super) fn execute(
    prepared: Prepared,
    control: Arc<dyn SelectorControl>,
    cancel: Cancellation,
    ready: SyncSender<Result<(), ClientError>>,
) -> Result<(), ClientError> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| ClientError::EngineStart)?;
    let result = runtime.block_on(async {
        tokio::select! {
            biased;
            () = cancel.cancelled() => Ok(()),
            result = run(prepared, control, ready) => result,
        }
    });
    runtime.shutdown_background();
    result
}

async fn run(
    prepared: Prepared,
    control: Arc<dyn SelectorControl>,
    ready: SyncSender<Result<(), ClientError>>,
) -> Result<(), ClientError> {
    let mut observer = Observer::new(prepared, control)?;
    let readiness = tokio::time::timeout(Duration::from_secs(2), observer.control.ready())
        .await
        .unwrap_or(Err(ClientError::EngineReadiness));
    let _ = ready.send(readiness);
    readiness?;
    let observatory = observer
        .prepared
        .graph
        .observatory
        .as_ref()
        .ok_or(ClientError::InvalidInput)?;
    let concurrent = observatory.enable_concurrency;
    let interval = observatory.interval;
    loop {
        if concurrent {
            observer.concurrent_round().await?;
        } else {
            observer.serial_round(interval).await?;
        }
        if concurrent {
            tokio::time::sleep(interval).await;
        }
    }
}

impl Observer {
    fn new(prepared: Prepared, control: Arc<dyn SelectorControl>) -> Result<Self, ClientError> {
        let probes = prepared
            .endpoints
            .iter()
            .map(|endpoint| {
                let client = if let Some(address) = endpoint.address {
                    let proxy = reqwest::Proxy::all(format!(
                        "socks5h://observer:{}@{}",
                        prepared.secret.expose(),
                        address
                    ))
                    .map_err(|_| ClientError::EngineStart)?;
                    Some(
                        probe_builder()?
                            .http1_only()
                            .pool_max_idle_per_host(0)
                            .proxy(proxy)
                            .timeout(Duration::from_secs(5))
                            .build()
                            .map_err(|_| ClientError::EngineStart)?,
                    )
                } else {
                    None
                };
                Ok(Probe {
                    tag: endpoint.tag.clone(),
                    client,
                })
            })
            .collect::<Result<Vec<_>, ClientError>>()?;
        let mut selected = HashMap::new();
        for (index, group) in prepared.graph.groups.iter().enumerate() {
            if matches!(group.strategy, GraphStrategy::LeastPing { .. }) {
                let target = select_least_ping(&prepared.graph, group, &[])?;
                let vpn_domain::client::GraphTarget::Outbound(tag) = target else {
                    return Err(ClientError::InvalidInput);
                };
                selected.insert(
                    graph::group_name(index),
                    graph::member_name(&prepared.graph, &tag)?,
                );
            }
        }
        Ok(Self {
            prepared,
            control,
            probes,
            reports: Vec::new(),
            selected,
        })
    }

    async fn serial_round(&mut self, interval: Duration) -> Result<(), ClientError> {
        if self.probes.is_empty() {
            tokio::time::sleep(interval).await;
            return Ok(());
        }
        for index in 0..self.probes.len() {
            let tag = self.probes[index].tag.clone();
            let delay = measure(self.probes[index].client.as_ref(), self.url()?).await;
            self.record(&tag, delay).await?;
            tokio::time::sleep(interval).await;
        }
        Ok(())
    }

    async fn concurrent_round(&mut self) -> Result<(), ClientError> {
        let mut pending = JoinSet::new();
        for probe in &self.probes {
            let tag = probe.tag.clone();
            let client = probe.client.clone();
            let url = self.url()?.to_owned();
            pending.spawn(async move { (tag, measure(client.as_ref(), &url).await) });
        }
        while let Some(result) = pending.join_next().await {
            let (tag, delay) = result.map_err(|_| ClientError::EngineStopped)?;
            self.record(&tag, delay).await?;
        }
        Ok(())
    }

    fn url(&self) -> Result<&str, ClientError> {
        self.prepared
            .graph
            .observatory
            .as_ref()
            .map(|observatory| observatory.probe_url.expose())
            .ok_or(ClientError::InvalidInput)
    }

    async fn record(&mut self, tag: &str, delay: Option<Duration>) -> Result<(), ClientError> {
        record_graph_probe(&mut self.reports, tag, delay)?;
        let mut changes = Vec::new();
        for (index, group) in self.prepared.graph.groups.iter().enumerate() {
            if matches!(group.strategy, GraphStrategy::LeastPing { .. }) {
                let target = select_least_ping(&self.prepared.graph, group, &self.reports)?;
                let vpn_domain::client::GraphTarget::Outbound(tag) = target else {
                    return Err(ClientError::InvalidInput);
                };
                changes.push((
                    graph::group_name(index),
                    graph::member_name(&self.prepared.graph, &tag)?,
                ));
            }
        }
        for (group, selected) in changes {
            if self.selected.get(&group) == Some(&selected) {
                continue;
            }
            self.apply(&group, &selected).await?;
            self.selected.insert(group, selected);
        }
        Ok(())
    }

    async fn apply(&self, group: &str, selected: &str) -> Result<(), ClientError> {
        tokio::time::timeout(Duration::from_secs(2), self.control.select(group, selected))
            .await
            .unwrap_or(Err(ClientError::EngineReadiness))
    }
}

fn probe_builder() -> Result<reqwest::ClientBuilder, ClientError> {
    #[cfg(target_os = "android")]
    {
        crate::client_http::android_tls(client_builder())
    }
    #[cfg(not(target_os = "android"))]
    {
        Ok(client_builder().tls_backend_native())
    }
}

fn client_builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
}

async fn measure(client: Option<&reqwest::Client>, url: &str) -> Option<Duration> {
    let client = client?;
    let start = Instant::now();
    let response = client.get(url).send().await.ok()?;
    let elapsed = Duration::from_millis(u64::try_from(start.elapsed().as_millis()).ok()?);
    // Xray's observer accepts every HTTP status and stops timing at headers.
    drop(response);
    Some(elapsed)
}

#[cfg(test)]
mod tests;
