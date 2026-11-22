//! Android egui adapter. Each Activity owns a surface while the client core lives in the process.

mod input;
mod packet;
mod textures;

use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use client_ui::{BlazerView, UiHost};
use jni::objects::{Global, JByteArray, JClass, JString, JValue};
use jni::refs::Reference;
use jni::sys::{jboolean, jfloat, jint, jlong};
use jni::{Env, JavaVM, NativeMethod, jni_sig, jni_str, native_method};
use vpn_presentation::{Controller, localization::Key};

use super::bridge::{self, AttachedClient};
use input::{InputEvent, InputQueue};

static ACTIVE_SURFACE: Mutex<Option<Arc<Surface>>> = Mutex::new(None);
static NEXT_SURFACE: AtomicI64 = AtomicI64::new(1);

const _: NativeMethod = native_method! {
    java_type = "com.kobaproduction.blazer.EguiNative",
    static extern fn create(updates_available: jboolean) -> jlong,
};
const _: NativeMethod = native_method! {
    java_type = "com.kobaproduction.blazer.EguiNative",
    static extern fn render(handle: jlong, width: jint, height: jint, density: jfloat, now_nanos: jlong) -> [jbyte],
};
const _: NativeMethod = native_method! {
    java_type = "com.kobaproduction.blazer.EguiNative",
    static extern fn touch(handle: jlong, action: jint, pointer_id: jint, x: jfloat, y: jfloat),
};
const _: NativeMethod = native_method! {
    java_type = "com.kobaproduction.blazer.EguiNative",
    static extern fn text(handle: jlong, value: JString),
};
const _: NativeMethod = native_method! {
    java_type = "com.kobaproduction.blazer.EguiNative",
    static extern fn key(handle: jlong, keycode: jint, down: jboolean, meta_state: jint),
};
const _: NativeMethod = native_method! {
    java_type = "com.kobaproduction.blazer.EguiNative",
    static extern fn paste(handle: jlong, value: JString),
};
const _: NativeMethod = native_method! {
    java_type = "com.kobaproduction.blazer.EguiNative",
    static extern fn ime(handle: jlong, phase: jint, value: JString),
};
const _: NativeMethod = native_method! {
    java_type = "com.kobaproduction.blazer.EguiNative",
    name = "deleteSurrounding",
    static extern fn delete_surrounding(handle: jlong, before_chars: jint, after_chars: jint),
};
const _: NativeMethod = native_method! {
    java_type = "com.kobaproduction.blazer.EguiNative",
    name = "importedQr",
    static extern fn imported_qr(handle: jlong, value: JString),
};
const _: NativeMethod = native_method! {
    java_type = "com.kobaproduction.blazer.EguiNative",
    name = "surfaceReset",
    static extern fn surface_reset(handle: jlong),
};
const _: NativeMethod = native_method! {
    java_type = "com.kobaproduction.blazer.EguiNative",
    static extern fn destroy(handle: jlong),
};

struct Surface {
    java: Arc<JavaUi>,
    client: AttachedClient,
    dirty: Arc<AtomicBool>,
    input: Mutex<InputQueue>,
    renderer: Mutex<Option<Renderer>>,
    updates_available: bool,
}

struct JavaUi {
    handle: jlong,
    vm: JavaVM,
    class: Global<JClass<'static>>,
}

impl JavaUi {
    fn repaint(&self, delay: Duration) {
        let Ok(delay_ms) = jlong::try_from(delay.as_millis()) else {
            return;
        };
        let _ = self.vm.attach_current_thread(|env| {
            env.call_static_method(
                &*self.class,
                jni_str!("requestRender"),
                jni_sig!("(JJ)V"),
                &[JValue::Long(self.handle), JValue::Long(delay_ms)],
            )?;
            Ok::<_, jni::errors::Error>(())
        });
    }

    fn request_tunnel_start(&self) -> bool {
        self.vm
            .attach_current_thread(|env| {
                env.call_static_method(
                    &*self.class,
                    jni_str!("requestVpnConsent"),
                    jni_sig!("(J)Z"),
                    &[JValue::Long(self.handle)],
                )?
                .z()
            })
            .unwrap_or(false)
    }

    fn pick_qr(&self, title: &str) -> bool {
        self.vm
            .attach_current_thread(|env| {
                let title = env.new_string(title)?;
                env.call_static_method(
                    &*self.class,
                    jni_str!("pickQrImage"),
                    jni_sig!("(JLjava/lang/String;)Z"),
                    &[JValue::Long(self.handle), JValue::Object(title.as_ref())],
                )?
                .z()
            })
            .unwrap_or(false)
    }

    fn scan_qr_camera(&self, title: &str, hint: &str, cancel: &str, error: &str) -> bool {
        self.vm
            .attach_current_thread(|env| {
                let title = env.new_string(title)?;
                let hint = env.new_string(hint)?;
                let cancel = env.new_string(cancel)?;
                let error = env.new_string(error)?;
                env.call_static_method(
                    &*self.class,
                    jni_str!("scanQrCamera"),
                    jni_sig!("(JLjava/lang/String;Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;)Z"),
                    &[
                        JValue::Long(self.handle),
                        JValue::Object(title.as_ref()),
                        JValue::Object(hint.as_ref()),
                        JValue::Object(cancel.as_ref()),
                        JValue::Object(error.as_ref()),
                    ],
                )?
                .z()
            })
            .unwrap_or(false)
    }

    fn open_updates(&self, error: &str) {
        let _ = self.vm.attach_current_thread(|env| {
            let error = env.new_string(error)?;
            env.call_static_method(
                &*self.class,
                jni_str!("openUpdates"),
                jni_sig!("(JLjava/lang/String;)V"),
                &[JValue::Long(self.handle), JValue::Object(error.as_ref())],
            )?;
            Ok::<_, jni::errors::Error>(())
        });
    }
}

impl Surface {
    fn new_renderer(&self) -> Renderer {
        let context = egui::Context::default();
        let repaint = self.java.clone();
        context.set_request_repaint_callback(move |request| repaint.repaint(request.delay));
        let tunnel = self.java.clone();
        let qr = self.java.clone();
        let camera = self.java.clone();
        let updates = self.java.clone();
        let host = UiHost {
            request_tunnel_start: Some(Arc::new(move || tunnel.request_tunnel_start())),
            pick_qr_image: Some(Arc::new(move |title| {
                if qr.pick_qr(title) {
                    Ok(None)
                } else {
                    Err(Key::QrUnreadableImage)
                }
            })),
            scan_qr_camera: Some(Arc::new(move |title, hint, cancel, error| {
                camera.scan_qr_camera(title, hint, cancel, error)
            })),
            open_updates: self
                .updates_available
                .then(|| Arc::new(move |error: &str| updates.open_updates(error)) as _),
            ..UiHost::default()
        };
        let controller = Controller::new(self.client.clone(), self.dirty.clone());
        let view = BlazerView::new(&context, controller, host);
        Renderer {
            context,
            view,
            input: input::InputState::default(),
            textures: textures::Textures::default(),
        }
    }

    fn render(&self, frame: Frame) -> Option<Vec<u8>> {
        let mut state = self.renderer.lock().ok()?;
        let renderer = state.get_or_insert_with(|| self.new_renderer());
        let incoming = std::mem::take(&mut *self.input.lock().ok()?);
        renderer.render(frame, incoming)
    }
}

struct Renderer {
    context: egui::Context,
    view: BlazerView,
    input: input::InputState,
    textures: textures::Textures,
}

#[derive(Clone, Copy)]
struct Frame {
    size_pixels: egui::Vec2,
    density: f32,
    now_seconds: f64,
}

impl Frame {
    fn new(width: jint, height: jint, density: jfloat, now_nanos: jlong) -> Option<Self> {
        if width <= 0 || height <= 0 || !density.is_finite() || density <= 0.0 {
            return None;
        }
        Some(Self {
            size_pixels: egui::vec2(width as f32, height as f32),
            density,
            now_seconds: now_nanos as f64 * 1e-9,
        })
    }
}

impl Renderer {
    fn render(&mut self, frame: Frame, mut incoming: InputQueue) -> Option<Vec<u8>> {
        let pixels_per_point = frame.density * self.context.zoom_factor();
        let raw = self.input.take_raw(&mut incoming, frame, pixels_per_point);
        if let Some(qr) = incoming.qr {
            self.view.prefill_import(qr);
        }
        let mut output = self.context.run_ui(raw, |ui| self.view.ui(ui));
        if self.textures.prepare(&mut output.textures_delta).is_none() {
            output.textures_delta.clear();
            self.textures.request_replay();
            return None;
        }
        let packet = packet::encode(
            &self.context,
            output,
            frame,
            self.input.take_paste_request(),
        );
        if packet.is_none() {
            self.textures.request_replay();
        }
        packet
    }
}

fn create(
    env: &mut Env<'_>,
    class: JClass<'_>,
    updates_available: jboolean,
) -> jni::errors::Result<jlong> {
    let Some(client) = bridge::attach_client() else {
        return Ok(0);
    };
    let handle = NEXT_SURFACE.fetch_add(1, Ordering::Relaxed);
    let java = JavaUi {
        handle,
        vm: env.get_java_vm()?,
        class: env.new_global_ref(&class)?,
    };
    let Ok(mut active) = ACTIVE_SURFACE.lock() else {
        return Ok(0);
    };
    *active = Some(Arc::new(Surface {
        java: Arc::new(java),
        client,
        dirty: Arc::new(AtomicBool::new(true)),
        input: Mutex::new(InputQueue::default()),
        renderer: Mutex::new(None),
        updates_available,
    }));
    Ok(handle)
}

fn current_surface(handle: jlong) -> Option<Arc<Surface>> {
    ACTIVE_SURFACE
        .lock()
        .ok()?
        .as_ref()
        .filter(|surface| surface.java.handle == handle)
        .cloned()
}

pub(super) fn wake_from_client_core() {
    let surface = ACTIVE_SURFACE.lock().ok().and_then(|active| active.clone());
    if let Some(surface) = surface {
        surface.dirty.store(true, Ordering::Release);
        surface.java.repaint(Duration::ZERO);
    }
}

fn render<'local>(
    env: &mut Env<'local>,
    _: JClass<'local>,
    handle: jlong,
    width: jint,
    height: jint,
    density: jfloat,
    now_nanos: jlong,
) -> jni::errors::Result<JByteArray<'local>> {
    let data = Frame::new(width, height, density, now_nanos)
        .zip(current_surface(handle))
        .and_then(|(frame, surface)| surface.render(frame));
    data.map_or_else(
        || Ok(JByteArray::null()),
        |data| env.byte_array_from_slice(&data),
    )
}

fn surface_reset(_: &mut Env<'_>, _: JClass<'_>, handle: jlong) -> jni::errors::Result<()> {
    if let Some(surface) = current_surface(handle)
        && let Ok(mut renderer) = surface.renderer.lock()
        && let Some(renderer) = renderer.as_mut()
    {
        renderer.textures.request_replay();
    }
    Ok(())
}

fn destroy(_: &mut Env<'_>, _: JClass<'_>, handle: jlong) -> jni::errors::Result<()> {
    if let Ok(mut active) = ACTIVE_SURFACE.lock()
        && active
            .as_ref()
            .is_some_and(|surface| surface.java.handle == handle)
    {
        *active = None;
    }
    Ok(())
}

fn push(handle: jlong, event: InputEvent) {
    if let Some(surface) = current_surface(handle)
        && let Ok(mut input) = surface.input.lock()
    {
        input.push(event);
    }
}

fn touch(
    _: &mut Env<'_>,
    _: JClass<'_>,
    handle: jlong,
    action: jint,
    pointer_id: jint,
    x: jfloat,
    y: jfloat,
) -> jni::errors::Result<()> {
    if let Some(event) = input::touch(action, pointer_id, x, y) {
        push(handle, event);
    }
    Ok(())
}

fn key(
    _: &mut Env<'_>,
    _: JClass<'_>,
    handle: jlong,
    code: jint,
    down: jboolean,
    meta: jint,
) -> jni::errors::Result<()> {
    push(handle, InputEvent::Key { code, down, meta });
    Ok(())
}

fn text(
    env: &mut Env<'_>,
    _: JClass<'_>,
    handle: jlong,
    value: JString<'_>,
) -> jni::errors::Result<()> {
    push_text(env, handle, &value, egui::Event::Text)
}

fn paste(
    env: &mut Env<'_>,
    _: JClass<'_>,
    handle: jlong,
    value: JString<'_>,
) -> jni::errors::Result<()> {
    push_text(env, handle, &value, egui::Event::Paste)
}

fn ime(
    env: &mut Env<'_>,
    _: JClass<'_>,
    handle: jlong,
    phase: jint,
    value: JString<'_>,
) -> jni::errors::Result<()> {
    match phase {
        1 => push_text(env, handle, &value, preedit),
        2 => push_text(env, handle, &value, |text| {
            egui::Event::Ime(egui::ImeEvent::Commit(text))
        }),
        3 => {
            push(handle, InputEvent::Egui(preedit(String::new())));
            Ok(())
        }
        _ => Ok(()),
    }
}

fn preedit(text: String) -> egui::Event {
    egui::Event::Ime(egui::ImeEvent::Preedit {
        text,
        active_range_chars: None,
    })
}

fn delete_surrounding(
    _: &mut Env<'_>,
    _: JClass<'_>,
    handle: jlong,
    before: jint,
    after: jint,
) -> jni::errors::Result<()> {
    let (Ok(before_chars), Ok(after_chars)) = (usize::try_from(before), usize::try_from(after))
    else {
        return Ok(());
    };
    let event = egui::ImeEvent::DeleteSurrounding {
        before_chars,
        after_chars,
    };
    push(handle, InputEvent::Egui(egui::Event::Ime(event)));
    Ok(())
}

fn push_text(
    env: &mut Env<'_>,
    handle: jlong,
    value: &JString<'_>,
    map: impl FnOnce(String) -> egui::Event,
) -> jni::errors::Result<()> {
    let text = value.mutf8_chars(env)?.to_string();
    if text.len() <= input::MAX_INPUT_BYTES {
        push(handle, InputEvent::Egui(map(text)));
    }
    Ok(())
}

fn imported_qr(
    env: &mut Env<'_>,
    _: JClass<'_>,
    handle: jlong,
    value: JString<'_>,
) -> jni::errors::Result<()> {
    let Some(surface) = current_surface(handle) else {
        return Ok(());
    };
    if value.is_null() {
        return Ok(());
    }
    let text = value.mutf8_chars(env)?.to_string();
    if !text.is_empty()
        && text.len() <= input::MAX_INPUT_BYTES
        && let Ok(mut input) = surface.input.lock()
    {
        input.qr = Some(text);
    }
    Ok(())
}
