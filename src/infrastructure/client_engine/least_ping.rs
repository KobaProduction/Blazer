//! Runtime integration for the Xray observatory contract. Its fixed-outbound
//! listeners share the running engine and require an unguessable session secret.

use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use vpn_application::client::{Cancellation, CancellationSignal};
use vpn_domain::client::{
    ClientError, GraphOutboundKind, ProxyGraph, ProxyProfile, ProxyProtocol, Secret,
};

use super::graph;

mod control;
mod worker;
pub use control::{SelectorControl, SelectorControlFuture};

struct Endpoint {
    tag: String,
    address: Option<SocketAddr>,
}

pub struct Prepared {
    graph: Arc<ProxyGraph>,
    #[cfg(not(target_os = "android"))]
    controller: Option<SocketAddr>,
    secret: Secret,
    endpoints: Vec<Endpoint>,
    reservations: Vec<TcpListener>,
}

impl Prepared {
    #[cfg(not(target_os = "android"))]
    pub(super) fn for_profile(profile: &ProxyProfile) -> Result<Option<Self>, ClientError> {
        Self::prepare(profile, false)
    }

    pub fn for_embedded_profile(profile: &ProxyProfile) -> Result<Option<Self>, ClientError> {
        Self::prepare(profile, true)
    }

    fn prepare(profile: &ProxyProfile, embedded: bool) -> Result<Option<Self>, ClientError> {
        if !graph::needs_observer(profile) {
            return Ok(None);
        }
        let ProxyProtocol::Graph(provider) = &profile.protocol else {
            return Err(ClientError::InvalidInput);
        };
        graph::validate(provider)?;
        let mut reservations = Vec::new();
        #[cfg(not(target_os = "android"))]
        let controller = if embedded {
            None
        } else {
            Some(reserve(&mut reservations)?)
        };
        #[cfg(target_os = "android")]
        let _ = embedded;
        let observatory = provider
            .observatory
            .as_ref()
            .ok_or(ClientError::UnsupportedFeature)?;
        let mut members = observatory.members.clone();
        members.sort();
        let mut endpoints = Vec::new();
        for tag in members {
            let outbound = provider
                .outbounds
                .iter()
                .find(|outbound| outbound.tag == tag)
                .ok_or(ClientError::InvalidInput)?;
            let address = if matches!(outbound.kind, GraphOutboundKind::Block) {
                None
            } else {
                Some(reserve(&mut reservations)?)
            };
            endpoints.push(Endpoint { tag, address });
        }
        Ok(Some(Self {
            graph: Arc::new((**provider).clone()),
            #[cfg(not(target_os = "android"))]
            controller,
            secret: Secret::new(format!(
                "{}{}",
                uuid::Uuid::new_v4().simple(),
                uuid::Uuid::new_v4().simple()
            )),
            endpoints,
            reservations,
        }))
    }

    #[cfg(not(target_os = "android"))]
    pub(super) fn decorate(&self, records: &mut Vec<Value>) -> Result<(), ClientError> {
        let address = self.controller.ok_or(ClientError::UnsupportedFeature)?;
        records.push(
            json!({"clash_api_address": address.to_string(), "secret": self.secret.expose()}),
        );
        self.decorate_embedded(records)
    }

    pub fn decorate_embedded(&self, records: &mut Vec<Value>) -> Result<(), ClientError> {
        for endpoint in &self.endpoints {
            let Some(address) = endpoint.address else {
                continue;
            };
            let chain = graph::member_name(&self.graph, &endpoint.tag)?;
            records.push(json!({"address": address.to_string(),
                "protocol": {"type": "socks", "username": "observer", "password": self.secret.expose()},
                "rules": [{"conditions": {"any_destination": true}, "action": "allow", "client_chain": chain}]}));
        }
        Ok(())
    }

    pub fn release_ports(&mut self) {
        self.reservations.clear();
    }

    #[cfg(not(target_os = "android"))]
    pub(super) fn start(self, cancel: &Cancellation) -> Result<Monitor, ClientError> {
        let address = self.controller.ok_or(ClientError::UnsupportedFeature)?;
        let control = Arc::new(control::HttpSelectorControl::new(
            address,
            self.secret.clone(),
        )?);
        self.start_with_control(cancel, control)
    }

    pub fn start_with_control(
        self,
        cancel: &Cancellation,
        control: Arc<dyn SelectorControl>,
    ) -> Result<Monitor, ClientError> {
        let signal = Arc::new(CancellationSignal::default());
        let stopped = Cancellation::new(Arc::clone(&signal), 1);
        let status = Arc::new(Mutex::new(None));
        let finished_status = Arc::clone(&status);
        let (ready, readiness) = mpsc::sync_channel(1);
        let (done, completion) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("blazer-observatory".into())
            .spawn(move || {
                let result = worker::execute(self, control, stopped, ready);
                if let Ok(mut status) = finished_status.lock() {
                    *status = Some(result);
                }
                let _ = done.send(());
            })
            .map_err(|_| ClientError::EngineStart)?;
        let mut monitor = Monitor {
            signal,
            thread: Some(thread),
            completion,
            status,
        };
        if let Err(error) = await_ready(readiness, cancel) {
            monitor.stop()?;
            return Err(error);
        }
        Ok(monitor)
    }
}

pub struct Monitor {
    signal: Arc<CancellationSignal>,
    thread: Option<JoinHandle<()>>,
    completion: mpsc::Receiver<()>,
    status: Arc<Mutex<Option<Result<(), ClientError>>>>,
}

impl Monitor {
    pub fn is_finished(&self) -> bool {
        self.thread.as_ref().is_none_or(JoinHandle::is_finished)
    }

    pub fn failure(&self) -> ClientError {
        self.status
            .lock()
            .ok()
            .and_then(|status| {
                status
                    .as_ref()
                    .and_then(|result| result.as_ref().err())
                    .copied()
            })
            .unwrap_or(ClientError::EngineStopped)
    }

    pub fn stop(&mut self) -> Result<(), ClientError> {
        self.signal.cancel(1);
        if self.thread.is_none() {
            return Ok(());
        }
        if !self.is_finished()
            && self
                .completion
                .recv_timeout(Duration::from_secs(2))
                .is_err()
            && !self.is_finished()
        {
            return Err(ClientError::EngineStopped);
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        Ok(())
    }
}

impl Drop for Monitor {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

fn reserve(reservations: &mut Vec<TcpListener>) -> Result<SocketAddr, ClientError> {
    let listener =
        TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).map_err(|_| ClientError::EngineStart)?;
    let address = listener
        .local_addr()
        .map_err(|_| ClientError::EngineStart)?;
    reservations.push(listener);
    Ok(address)
}

fn await_ready(
    receiver: mpsc::Receiver<Result<(), ClientError>>,
    cancel: &Cancellation,
) -> Result<(), ClientError> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        cancel.check()?;
        match receiver.recv_timeout(Duration::from_millis(20)) {
            Ok(result) => return result,
            Err(mpsc::RecvTimeoutError::Timeout) if Instant::now() < deadline => {}
            _ => return Err(ClientError::EngineReadiness),
        }
    }
}
