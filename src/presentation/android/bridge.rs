use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use jni::objects::{Global, JByteArray, JClass, JString};
use jni::sys::{JNI_FALSE, JNI_TRUE, jboolean, jint, jlong};
use jni::{Env, JavaVM, NativeMethod, jni_sig, jni_str, native_method};
use serde::Deserialize;
use serde_json::{Value, json};
use vpn_backend::{
    ClientHandle, PeriodicRefresh, RefreshControl, RefreshStatus, SubscriptionRefreshMode,
};
use vpn_infrastructure::android::{AndroidEngine, AndroidPlatform, AndroidProtection};
use vpn_presentation::Controller;
use vpn_presentation::import::{self, ImportKind};
use vpn_presentation::localization::{Key, text};
use vpn_presentation::model::{self, ErrorNotice, Intent};
use vpn_presentation::session::{ConnectionMode, Language};

static SESSION: Mutex<Option<AndroidSession>> = Mutex::new(None);
const MAX_COMMAND_BYTES: usize = 2 * 1024 * 1024;

struct AndroidSession {
    controller: Controller,
    refresh: RefreshControl,
    periodic: Option<PeriodicRefresh>,
}

const _: NativeMethod = native_method! {
    java_type = "com.kobaproduction.vpnclient.NativeSession",
    static extern fn open(data_dir: JString, shoes_packaged: jboolean) -> jboolean,
};

const _: NativeMethod = native_method! {
    java_type = "com.kobaproduction.vpnclient.NativeSession",
    static extern fn snapshot() -> JString,
};

const _: NativeMethod = native_method! {
    java_type = "com.kobaproduction.vpnclient.NativeSession",
    static extern fn dispatch(request: JString) -> jboolean,
};

const _: NativeMethod = native_method! {
    java_type = "com.kobaproduction.vpnclient.NativeSession",
    name = "decodeQrImage",
    static extern fn decode_qr_image(image: [jbyte]) -> JString,
};

const _: NativeMethod = native_method! {
    java_type = "com.kobaproduction.vpnclient.NativeSession",
    name = "decodeQrFrame",
    static extern fn decode_qr_frame(frame: [jbyte], width: jint, height: jint) -> JString,
};

const _: NativeMethod = native_method! {
    java_type = "com.kobaproduction.vpnclient.NativeSession",
    name = "requestTunnelStart",
    static extern fn request_tunnel_start() -> jlong,
};

const _: NativeMethod = native_method! {
    java_type = "com.kobaproduction.vpnclient.NativeSession",
    name = "tunnelStartOutcome",
    static extern fn tunnel_start_outcome(request: jlong) -> jint,
};

const _: NativeMethod = native_method! {
    java_type = "com.kobaproduction.vpnclient.NativeSession",
    name = "setForeground",
    static extern fn set_foreground(visible: jboolean),
};

const _: NativeMethod = native_method! {
    java_type = "com.kobaproduction.vpnclient.NativeSession",
    name = "startPeriodicRefresh",
    static extern fn start_periodic_refresh() -> jlong,
};

const _: NativeMethod = native_method! {
    java_type = "com.kobaproduction.vpnclient.NativeSession",
    name = "cancelPeriodicRefresh",
    static extern fn cancel_periodic_refresh(id: jlong),
};

const _: NativeMethod = native_method! {
    java_type = "com.kobaproduction.vpnclient.NativeSession",
    name = "refreshSchedule",
    static extern fn refresh_schedule() -> JString,
};

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum Action {
    Primary,
    Disconnect,
    Import { title: String, text: String },
    AutoImport { text: String },
    Subscribe { title: String, url: String },
    Refresh { uuid: String },
    Select { uuid: String },
    ToggleLanguage,
}

impl Action {
    fn intent(self, controller: &Controller) -> Option<Intent> {
        let language = controller.snapshot().language;
        Some(match self {
            Self::Primary => {
                return model::primary_action(controller.snapshot(), controller.is_saving_mode()).1;
            }
            Self::Disconnect => Intent::Disconnect,
            Self::Import { title, text } => {
                ImportKind::Configuration.intent(model::locale(language), &title, &text)
            }
            Self::AutoImport { text } => {
                import::detect(&text).intent(model::locale(language), "", &text)
            }
            Self::Subscribe { title, url } => {
                ImportKind::Subscription.intent(model::locale(language), &title, &url)
            }
            Self::Refresh { uuid } => Intent::RefreshSubscription(uuid),
            Self::Select { uuid } => Intent::SelectProfile(uuid),
            Self::ToggleLanguage => Intent::ChangeLanguage(match language {
                Language::Russian => Language::English,
                Language::English => Language::Russian,
            }),
        })
    }
}

fn open<'local>(
    env: &mut Env<'local>,
    class: JClass<'local>,
    data_dir: JString<'local>,
    shoes_packaged: jboolean,
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
            Box::new(AndroidEngine::new(hooks, shoes_packaged)),
            SubscriptionRefreshMode::Platform,
        ) else {
            return Ok(JNI_FALSE);
        };
        let dirty = Arc::new(AtomicBool::new(true));
        let signal = dirty.clone();
        client.set_notifier(Arc::new(move || {
            signal.store(true, Ordering::Release);
            notify_observer(&vm, &observer);
        }));
        let refresh = client.refresh_control();
        *session = Some(AndroidSession {
            controller: Controller::new(client, dirty),
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

fn snapshot<'local>(
    env: &mut Env<'local>,
    _class: JClass<'local>,
) -> jni::errors::Result<JString<'local>> {
    let result = with_controller(|controller| {
        controller.refresh();
        view(controller)
    });
    let payload = result.map_or_else(|| "{}".to_owned(), |view| view.to_string());
    env.new_string(&payload)
}

fn dispatch<'local>(
    env: &mut Env<'local>,
    _class: JClass<'local>,
    request: JString<'local>,
) -> jni::errors::Result<jboolean> {
    let request = request.mutf8_chars(env)?.to_string();
    if request.len() > MAX_COMMAND_BYTES {
        return Ok(JNI_FALSE);
    }
    let Ok(action) = serde_json::from_str::<Action>(&request) else {
        return Ok(JNI_FALSE);
    };
    let accepted = with_controller(|controller| {
        action
            .intent(controller)
            .is_some_and(|intent| controller.dispatch(intent))
    });
    Ok(if accepted == Some(true) {
        JNI_TRUE
    } else {
        JNI_FALSE
    })
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

fn view(controller: &Controller) -> Value {
    let snapshot = controller.snapshot();
    let locale = model::locale(snapshot.language);
    let label = |key| text(locale, key);
    let mut profiles: Vec<_> = snapshot.profiles.iter().map(|profile| json!({
        "uuid": &profile.profile_uuid,
        "name": &profile.name,
        "protocol": &profile.protocol,
        "selected": snapshot.selected_profile_uuid.as_deref() == Some(&profile.profile_uuid),
    })).collect();
    profiles.extend(
        snapshot
            .subscriptions
            .iter()
            .filter_map(|subscription| {
                snapshot
                    .unavailable_profiles
                    .get(&subscription.subscription_uuid)
            })
            .flatten()
            .map(|profile| {
                json!({
                    "name": model::unavailable_profile_name(locale, profile),
                    "protocol": profile.protocol.as_deref().unwrap_or_default(),
                    "unavailable_reason": model::unavailable_profile_reason(locale, profile.reason),
                })
            }),
    );
    let subscriptions: Vec<_> = snapshot
        .subscriptions
        .iter()
        .map(|item| {
            json!({
                "uuid": &item.subscription_uuid,
                "title": &item.title,
                "count": item.profile_count,
                "error": item.last_error.map(|error| error_text(ErrorNotice::Client(error), locale)),
            })
        })
        .collect();
    let state = model::state_key(snapshot.connection, snapshot.active_mode);
    let (action_label, action) = model::primary_action(snapshot, controller.is_saving_mode());
    json!({
        "busy": snapshot.busy,
        "error": controller.error().map(|error| error_text(error, locale)),
        "status": label(state),
        "primary_action": label(action_label),
        "primary_enabled": action.is_some(),
        "request_vpn": matches!(action, Some(Intent::Connect(ConnectionMode::Tun))),
        "mode_available": snapshot.mode_capabilities.supports(snapshot.selected_mode),
        "language": match snapshot.language { Language::English => "EN", Language::Russian => "RU" },
        "profiles": profiles,
        "subscriptions": subscriptions,
        "labels": labels(snapshot.language),
    })
}

fn error_text(
    error: ErrorNotice,
    locale: vpn_presentation::localization::Locale,
) -> std::borrow::Cow<'static, str> {
    error.text_with_code(locale, error.key())
}

fn labels(language: Language) -> Value {
    let locale = model::locale(language);
    let label = |key| text(locale, key);
    json!({
        "title": label(Key::AppName), "profiles": label(Key::Profiles),
        "subscriptions": label(Key::Subscriptions), "unavailable": label(Key::ModeUnavailable),
        "add_subscription": label(Key::AddSubscription), "import": label(Key::AddConfiguration),
        "clipboard": label(Key::AddClipboard), "clipboard_empty": label(Key::ClipboardEmpty),
        "clipboard_large": label(Key::ClipboardTextTooLarge), "qr": label(Key::AddQr),
        "qr_file": label(Key::QrOpenFile), "qr_screenshot": label(Key::QrScreenshot),
        "qr_camera": label(Key::QrCamera), "qr_camera_help": label(Key::QrCameraHelp),
        "qr_camera_unavailable": label(Key::QrCameraUnavailable),
        "qr_image_error": label(Key::QrUnreadableImage), "qr_image_large": label(Key::QrImageTooLarge),
        "qr_screenshot_help": label(Key::QrScreenshotMobileHelp), "import_problem": label(Key::ImportProblem),
        "import_text": label(Key::ConfigurationText), "configuration_name": label(Key::ConfigurationName),
        "name": label(Key::SubscriptionName), "url": label(Key::SubscriptionUrl),
        "empty_profiles": label(Key::NoProfiles), "empty_subscriptions": label(Key::NoSubscriptions),
        "refresh": label(Key::Refresh), "select": label(Key::ActiveProfile),
        "selected": label(Key::Selected), "cancel": label(Key::Cancel), "save": label(Key::Save),
        "connect": label(Key::Connect), "busy": label(Key::Busy),
    })
}
