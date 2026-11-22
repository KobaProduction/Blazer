use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use client_core::{
    ClientHandle, PeriodicRefresh, RefreshControl, RefreshStatus, SubscriptionRefreshMode,
};
use jni::objects::{Global, JByteArray, JClass, JString};
use jni::sys::{JNI_FALSE, JNI_TRUE, jboolean, jint, jlong};
use jni::{Env, JavaVM, NativeMethod, jni_sig, jni_str, native_method};
use serde_json::json;
use vpn_infrastructure::android::{AndroidEngine, AndroidPlatform, AndroidProtection};
use vpn_presentation::Controller;
use vpn_presentation::localization::text;
use vpn_presentation::model::{self, Intent};
use vpn_presentation::session::{
    ClientCommand, ClientError, ClientSession, ClientSnapshot, ConnectionMode, Language, RequestId,
    SubmitError,
};

static SESSION: Mutex<Option<AndroidSession>> = Mutex::new(None);

#[cfg(all(feature = "acceptance", not(debug_assertions)))]
compile_error!("The Android acceptance bridge is restricted to local debug builds");
#[cfg(all(feature = "acceptance", debug_assertions))]
#[path = "acceptance.rs"]
mod acceptance;

struct AndroidSession {
    client: Arc<ClientHandle>,
    controller: Controller,
    refresh: RefreshControl,
    periodic: Option<PeriodicRefresh>,
}

// The process owns the client core. Closing a view releases only its attachment.
#[derive(Clone)]
pub(super) struct AttachedClient(Arc<ClientHandle>);

impl ClientSession for AttachedClient {
    fn snapshot(&self) -> ClientSnapshot {
        self.0.snapshot()
    }

    fn send(&self, command: ClientCommand) -> Result<RequestId, SubmitError> {
        self.0.send(command)
    }

    fn shutdown(&mut self) -> Result<(), ClientError> {
        Ok(())
    }
}

pub(super) fn attach_client() -> Option<AttachedClient> {
    with_session(|session| AttachedClient(session.client.clone()))
}

const _: NativeMethod = native_method! {
    java_type = "com.kobaproduction.blazer.NativeSession",
    static extern fn open(data_dir: JString, blaze_core_packaged: jboolean) -> jboolean,
};

const _: NativeMethod = native_method! {
    java_type = "com.kobaproduction.blazer.NativeSession",
    static extern fn disconnect(),
};

const _: NativeMethod = native_method! {
    java_type = "com.kobaproduction.blazer.NativeSession",
    name = "decodeQrImage",
    static extern fn decode_qr_image(image: [jbyte]) -> JString,
};

const _: NativeMethod = native_method! {
    java_type = "com.kobaproduction.blazer.NativeSession",
    name = "decodeQrFrame",
    static extern fn decode_qr_frame(frame: [jbyte], width: jint, height: jint) -> JString,
};

const _: NativeMethod = native_method! {
    java_type = "com.kobaproduction.blazer.NativeSession",
    name = "requestTunnelStart",
    static extern fn request_tunnel_start() -> jlong,
};

const _: NativeMethod = native_method! {
    java_type = "com.kobaproduction.blazer.NativeSession",
    name = "tunnelStartOutcome",
    static extern fn tunnel_start_outcome(request: jlong) -> jint,
};

const _: NativeMethod = native_method! {
    java_type = "com.kobaproduction.blazer.NativeSession",
    name = "setForeground",
    static extern fn set_foreground(visible: jboolean),
};

const _: NativeMethod = native_method! {
    java_type = "com.kobaproduction.blazer.NativeSession",
    name = "startPeriodicRefresh",
    static extern fn start_periodic_refresh() -> jlong,
};

const _: NativeMethod = native_method! {
    java_type = "com.kobaproduction.blazer.NativeSession",
    name = "cancelPeriodicRefresh",
    static extern fn cancel_periodic_refresh(id: jlong),
};

const _: NativeMethod = native_method! {
    java_type = "com.kobaproduction.blazer.NativeSession",
    name = "refreshSchedule",
    static extern fn refresh_schedule() -> JString,
};

fn open<'local>(
    env: &mut Env<'local>,
    class: JClass<'local>,
    data_dir: JString<'local>,
    blaze_core_packaged: jboolean,
) -> jni::errors::Result<jboolean> {
    let Ok(mut session) = SESSION.lock() else {
        return Ok(JNI_FALSE);
    };
    if session.is_none() {
        let dir = PathBuf::from(data_dir.mutf8_chars(env)?.to_string());
        let hooks = Arc::new(AndroidPlatform::new(env, &class)?);
        let vm = env.get_java_vm()?;
        let observer = env.new_global_ref(&class)?;
        let Ok(client) = ClientHandle::spawn_with_platform(
            dir,
            Box::new(AndroidProtection(hooks.clone())),
            Box::new(AndroidEngine::new(hooks, blaze_core_packaged)),
            SubscriptionRefreshMode::Platform,
        ) else {
            return Ok(JNI_FALSE);
        };
        let dirty = Arc::new(AtomicBool::new(true));
        let signal = dirty.clone();
        client.set_notifier(Arc::new(move || {
            signal.store(true, Ordering::Release);
            notify_observer(&vm, &observer);
            super::renderer::wake_from_client_core();
        }));
        let refresh = client.refresh_control();
        let client = Arc::new(client);
        *session = Some(AndroidSession {
            controller: Controller::new(AttachedClient(client.clone()), dirty),
            client,
            refresh,
            periodic: None,
        });
    }
    Ok(JNI_TRUE)
}

fn notify_observer(vm: &JavaVM, observer: &Global<JClass<'static>>) {
    let _ = vm.attach_current_thread(|env| {
        env.call_static_method(
            &**observer,
            jni_str!("onNativeUpdate"),
            jni_sig!("()V"),
            &[],
        )?;
        Ok::<_, jni::errors::Error>(())
    });
}

fn disconnect(_: &mut Env<'_>, _: JClass<'_>) -> jni::errors::Result<()> {
    let _ = with_controller(|controller| controller.dispatch(Intent::Disconnect));
    Ok(())
}
fn with_controller<T>(f: impl FnOnce(&mut Controller) -> T) -> Option<T> {
    with_session(|session| f(&mut session.controller))
}

fn decode_qr_image<'local>(
    env: &mut Env<'local>,
    _: JClass<'local>,
    image: JByteArray<'local>,
) -> jni::errors::Result<JString<'local>> {
    let length = image.len(env)?;
    let result = if length > vpn_image_import::MAX_IMAGE_BYTES {
        Err(vpn_image_import::Error::TooLarge)
    } else {
        vpn_image_import::image(&env.convert_byte_array(&image)?)
    };
    let value = match result {
        Ok(payload) => json!({"text": payload}),
        Err(error) => {
            let language = with_controller(|controller| controller.snapshot().language)
                .unwrap_or(Language::Russian);
            json!({"error": text(model::locale(language), error.key())})
        }
    };
    env.new_string(value.to_string())
}

fn decode_qr_frame<'local>(
    env: &mut Env<'local>,
    _: JClass<'local>,
    frame: JByteArray<'local>,
    width: jint,
    height: jint,
) -> jni::errors::Result<JString<'local>> {
    let width = usize::try_from(width).unwrap_or_default();
    let height = usize::try_from(height).unwrap_or_default();
    let Ok(count) = vpn_image_import::check_dimensions(width, height) else {
        return env.new_string("");
    };
    let length = frame.len(env)?;
    if length < count || length > count.saturating_mul(2) {
        return env.new_string("");
    }
    let bytes = env.convert_byte_array(&frame)?;
    let result = decode_camera_luma(width, height, &bytes[..count]).unwrap_or_default();
    env.new_string(result)
}

fn decode_camera_luma(
    width: usize,
    height: usize,
    gray: &[u8],
) -> Result<String, vpn_image_import::Error> {
    let side = 2 * width.min(height) / 3;
    let left = (width - side) / 2;
    let top = (height - side) / 2;
    let cropped: Vec<_> = gray
        .chunks_exact(width)
        .skip(top)
        .take(side)
        .flat_map(|row| row[left..left + side].iter().copied())
        .collect();
    vpn_image_import::gray(side, side, &cropped)
        .or_else(|_| vpn_image_import::gray(width, height, gray))
}

fn with_session<T>(f: impl FnOnce(&mut AndroidSession) -> T) -> Option<T> {
    SESSION.lock().ok()?.as_mut().map(f)
}

fn request_tunnel_start(_: &mut Env<'_>, _: JClass<'_>) -> jni::errors::Result<jlong> {
    Ok(with_controller(|controller| {
        controller.refresh();
        let snapshot = controller.snapshot();
        if snapshot.busy || snapshot.active_mode.is_some() || snapshot.cleanup_required {
            return None;
        }
        let request = controller.submit(Intent::Connect(ConnectionMode::Tun))?;
        jlong::try_from(request.0).ok()
    })
    .flatten()
    .unwrap_or(0))
}

fn tunnel_start_outcome(
    _: &mut Env<'_>,
    _: JClass<'_>,
    request: jlong,
) -> jni::errors::Result<jint> {
    let Ok(request) = u64::try_from(request) else {
        return Ok(0);
    };
    Ok(with_controller(|controller| {
        controller.refresh();
        let snapshot = controller.snapshot();
        if snapshot
            .completed_request
            .is_none_or(|completed| completed.0 < request)
        {
            return 0;
        }
        if snapshot.active_mode == Some(ConnectionMode::Tun) || snapshot.cleanup_required {
            1
        } else {
            -1
        }
    })
    .unwrap_or(0))
}

fn set_foreground(_: &mut Env<'_>, _: JClass<'_>, visible: jboolean) -> jni::errors::Result<()> {
    with_session(|session| session.refresh.set_foreground(visible));
    Ok(())
}

fn start_periodic_refresh(_: &mut Env<'_>, _: JClass<'_>) -> jni::errors::Result<jlong> {
    Ok(with_session(|session| {
        let request = session.refresh.periodic().ok()?;
        let id = jlong::try_from(request.id.0).ok()?;
        session.periodic = Some(request);
        Some(id)
    })
    .flatten()
    .unwrap_or(0))
}

fn cancel_periodic_refresh(_: &mut Env<'_>, _: JClass<'_>, id: jlong) -> jni::errors::Result<()> {
    with_session(|session| {
        if let Some(request) = &session.periodic
            && u64::try_from(id) == Ok(request.id.0)
        {
            request.cancel();
            session.periodic = None;
        }
    });
    Ok(())
}

fn refresh_schedule<'local>(
    env: &mut Env<'local>,
    _: JClass<'local>,
) -> jni::errors::Result<JString<'local>> {
    let status = with_session(|session| session.refresh.status()).unwrap_or_default();
    let payload = match status {
        RefreshStatus::Starting => json!({}),
        RefreshStatus::Failed => json!({ "failed": true }),
        RefreshStatus::Scheduled {
            next_unix,
            completed,
        } => json!({
            "ready": true,
            "deadline": next_unix,
            "completed": completed.map(|id| id.0),
        }),
    };
    env.new_string(payload.to_string())
}
