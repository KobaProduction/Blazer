//! Thin extension ABI; policy, HTTP and scheduling remain in shared client code.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

use safer_ffi::prelude::*;
use vpn_application::client::{Cancellation, CancellationSignal, ClientNormalizerPort};
use vpn_infrastructure::client_engine::{
    ObservatoryMonitor, PreparedObservatory, SelectorControl, SelectorControlFuture,
};
use vpn_infrastructure::client_normalize::SubscriptionNormalizer;
use zeroize::Zeroize;

const MAX_PAYLOAD: usize = 64 * 1024;
static NEXT_HANDLE: AtomicU64 = AtomicU64::new(1);
static SESSIONS: LazyLock<Mutex<HashMap<u64, Session>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

struct Session {
    prepared: Option<PreparedObservatory>,
    monitor: Option<ObservatoryMonitor>,
}

#[derive_ReprC]
#[repr(C)]
pub struct ObserverPreparation {
    pub handle: u64,
    pub code: i32,
    pub configuration: repr_c::Vec<u8>,
}

pub type ReadyCallback = extern "C" fn(u64) -> i32;
pub type SelectCallback = extern "C" fn(u64, *const u8, usize, *const u8, usize) -> i32;

struct NativeControl {
    generation: u64,
    ready: ReadyCallback,
    select: SelectCallback,
}

impl SelectorControl for NativeControl {
    fn ready(&self) -> SelectorControlFuture<'_> {
        let generation = self.generation;
        let callback = self.ready;
        Box::pin(async move {
            callback_result(tokio::task::spawn_blocking(move || callback(generation)).await)
        })
    }

    fn select<'a>(&'a self, group: &'a str, member: &'a str) -> SelectorControlFuture<'a> {
        let generation = self.generation;
        let callback = self.select;
        let group = group.as_bytes().to_vec();
        let member = member.as_bytes().to_vec();
        Box::pin(async move {
            callback_result(
                tokio::task::spawn_blocking(move || {
                    callback(
                        generation,
                        group.as_ptr(),
                        group.len(),
                        member.as_ptr(),
                        member.len(),
                    )
                })
                .await,
            )
        })
    }
}

fn callback_result(
    result: Result<i32, tokio::task::JoinError>,
) -> Result<(), vpn_application::client::ClientError> {
    match result {
        Ok(0) => Ok(()),
        Ok(-6) => Err(vpn_application::client::ClientError::UnsupportedFeature),
        _ => Err(vpn_application::client::ClientError::EngineReadiness),
    }
}

fn prepare(payload: &[u8]) -> Result<ObserverPreparation, i32> {
    if payload.len() > MAX_PAYLOAD {
        return Err(-1);
    }
    let request: serde_json::Value = serde_json::from_slice(payload).map_err(|_| -1)?;
    let source = request
        .get("observatoryProfile")
        .and_then(serde_json::Value::as_str)
        .ok_or(-1)?;
    let [profile]: [_; 1] = SubscriptionNormalizer
        .normalize(source.as_bytes())
        .map_err(|_| -1)?
        .try_into()
        .map_err(|_| -1)?;
    let prepared = PreparedObservatory::for_embedded_profile(&profile)
        .map_err(|_| -6)?
        .ok_or(-6)?;
    let mut records = request
        .get("records")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .ok_or(-1)?;
    prepared.decorate_embedded(&mut records).map_err(|_| -8)?;
    let configuration = serde_json::to_vec(&records).map_err(|_| -8)?;
    if configuration.len() > MAX_PAYLOAD {
        return Err(-1);
    }
    let handle = NEXT_HANDLE.fetch_add(1, Ordering::Relaxed);
    if handle == 0 {
        return Err(-8);
    }
    let mut sessions = SESSIONS.lock().map_err(|_| -8)?;
    if !sessions.is_empty() {
        return Err(-7);
    }
    sessions.insert(
        handle,
        Session {
            prepared: Some(prepared),
            monitor: None,
        },
    );
    Ok(ObserverPreparation {
        handle,
        code: 0,
        configuration: configuration.into(),
    })
}

#[ffi_export]
pub fn blazer_observer_prepare(payload: c_slice::Ref<'_, u8>) -> ObserverPreparation {
    prepare(payload.as_ref()).unwrap_or_else(|code| ObserverPreparation {
        handle: 0,
        code,
        configuration: Vec::new().into(),
    })
}

#[ffi_export]
pub fn blazer_observer_free_configuration(mut configuration: repr_c::Vec<u8>) {
    configuration.as_mut().as_mut().zeroize();
}

#[ffi_export]
pub fn blazer_observer_release_ports(handle: u64) -> i32 {
    let Ok(mut sessions) = SESSIONS.lock() else {
        return -8;
    };
    let Some(session) = sessions.get_mut(&handle) else {
        return -2;
    };
    let Some(prepared) = &mut session.prepared else {
        return -3;
    };
    prepared.release_ports();
    0
}

#[ffi_export]
pub fn blazer_observer_start(
    handle: u64,
    core_generation: u64,
    ready: ReadyCallback,
    select: SelectCallback,
) -> i32 {
    if core_generation == 0 {
        return -1;
    }
    let Ok(mut sessions) = SESSIONS.lock() else {
        return -8;
    };
    let Some(session) = sessions.get_mut(&handle) else {
        return -2;
    };
    let Some(prepared) = session.prepared.take() else {
        return -3;
    };
    let cancel = Cancellation::new(Arc::new(CancellationSignal::default()), 1);
    let control = Arc::new(NativeControl {
        generation: core_generation,
        ready,
        select,
    });
    match prepared.start_with_control(&cancel, control) {
        Ok(monitor) => {
            session.monitor = Some(monitor);
            0
        }
        Err(vpn_application::client::ClientError::UnsupportedFeature) => -6,
        Err(_) => -8,
    }
}

#[ffi_export]
pub fn blazer_observer_status(handle: u64) -> i32 {
    let Ok(sessions) = SESSIONS.lock() else {
        return -8;
    };
    let Some(session) = sessions.get(&handle) else {
        return -2;
    };
    match &session.monitor {
        Some(monitor) if monitor.is_finished() => -8,
        Some(_) => 1,
        None => 0,
    }
}

#[ffi_export]
pub fn blazer_observer_stop(handle: u64) -> i32 {
    let Ok(mut sessions) = SESSIONS.lock() else {
        return -8;
    };
    let Some(mut session) = sessions.remove(&handle) else {
        return -2;
    };
    if let Some(monitor) = &mut session.monitor
        && monitor.stop().is_err()
    {
        sessions.insert(handle, session);
        return -7;
    }
    0
}

#[cfg(feature = "headers")]
pub fn generate_header(path: &std::path::Path) -> std::io::Result<()> {
    safer_ffi::headers::builder().to_file(path)?.generate()
}

#[cfg(test)]
mod tests {
    use super::*;

    static GENERATION: AtomicU64 = AtomicU64::new(0);

    extern "C" fn ready(generation: u64) -> i32 {
        GENERATION.store(generation, Ordering::Relaxed);
        0
    }

    extern "C" fn select(generation: u64, _: *const u8, _: usize, _: *const u8, _: usize) -> i32 {
        if generation == 42 { 0 } else { -2 }
    }

    fn fixture() -> Vec<u8> {
        let source = serde_json::json!({
            "outbounds":[{"tag":"proxy", "protocol":"socks", "settings":{"servers":[{"address":"127.0.0.1","port":9}]}}],
            "routing":{"balancers":[{"tag":"least","selector":["proxy"],"strategy":{"type":"leastPing"}}],"rules":[]},
            "observatory":{"subjectSelector":["proxy"],"probeURL":"http://check.invalid/","probeInterval":"60s","enableConcurrency":true}
        });
        serde_json::to_vec(
            &serde_json::json!({"records":[], "observatoryProfile":source.to_string()}),
        )
        .unwrap()
    }

    #[test]
    fn extension_bridge_owns_configuration_and_fences_prepared_monitor_lifecycle() {
        let payload = fixture();
        let prepared = blazer_observer_prepare(payload.as_slice().into());
        assert_eq!(prepared.code, 0);
        assert_ne!(prepared.handle, 0);
        let records: serde_json::Value =
            serde_json::from_slice(prepared.configuration.as_ref().as_ref()).unwrap();
        assert_eq!(records.as_array().unwrap().len(), 1);
        assert!(records[0].get("clash_api_address").is_none());
        assert_eq!(records[0]["protocol"]["username"], "observer");
        blazer_observer_free_configuration(prepared.configuration);
        assert_eq!(blazer_observer_status(prepared.handle), 0);
        assert_eq!(blazer_observer_release_ports(prepared.handle), 0);
        assert_eq!(blazer_observer_start(prepared.handle, 42, ready, select), 0);
        assert_eq!(GENERATION.load(Ordering::Relaxed), 42);
        assert_eq!(blazer_observer_status(prepared.handle), 1);
        assert_eq!(blazer_observer_stop(prepared.handle), 0);
        assert_eq!(blazer_observer_status(prepared.handle), -2);
        assert_eq!(
            blazer_observer_start(prepared.handle, 43, ready, select),
            -2
        );
        assert_next_session(&payload, prepared.handle);
    }

    fn assert_next_session(payload: &[u8], previous: u64) {
        let next = blazer_observer_prepare(payload.into());
        assert_eq!(next.code, 0);
        assert_ne!(next.handle, previous);
        blazer_observer_free_configuration(next.configuration);
        assert_eq!(blazer_observer_stop(next.handle), 0);
        assert_eq!(blazer_observer_prepare(b"{}".as_slice().into()).code, -1);
    }
}
