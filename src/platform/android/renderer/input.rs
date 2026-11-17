use egui::{
    Event, Key, Modifiers, PointerButton, Pos2, RawInput, Rect, TouchDeviceId, TouchId, TouchPhase,
    ViewportId,
};

use super::Frame;

pub(super) const MAX_INPUT_BYTES: usize = 2 * 1024 * 1024;
const MAX_EVENTS: usize = 128;

pub(super) enum InputEvent {
    Touch {
        phase: TouchPhase,
        id: u64,
        pixels: Pos2,
    },
    Key {
        code: i32,
        down: bool,
        meta: i32,
    },
    Egui(Event),
}

#[derive(Default)]
pub(super) struct InputQueue {
    events: Vec<InputEvent>,
    text_bytes: usize,
    reset_pointer: bool,
    pub qr: Option<String>,
}

impl InputQueue {
    pub(super) fn push(&mut self, event: InputEvent) {
        let bytes = event.text_bytes();
        if self.text_bytes.saturating_add(bytes) > MAX_INPUT_BYTES {
            return;
        }
        if self.events.len() >= MAX_EVENTS {
            self.events.clear();
            self.text_bytes = 0;
            self.reset_pointer = true;
        }
        self.text_bytes += bytes;
        self.events.push(event);
    }
}

impl InputEvent {
    fn text_bytes(&self) -> usize {
        match self {
            Self::Egui(
                Event::Text(text)
                | Event::Paste(text)
                | Event::Ime(egui::ImeEvent::Commit(text) | egui::ImeEvent::Preedit { text, .. }),
            ) => text.len(),
            _ => 0,
        }
    }
}

pub(super) fn touch(action: i32, id: i32, x: f32, y: f32) -> Option<InputEvent> {
    let id = u64::try_from(id).ok()?;
    if !x.is_finite() || !y.is_finite() {
        return None;
    }
    let phase = match action {
        0 => TouchPhase::Start,
        1 => TouchPhase::End,
        2 => TouchPhase::Move,
        3 => TouchPhase::Cancel,
        _ => return None,
    };
    Some(InputEvent::Touch {
        phase,
        id,
        pixels: egui::pos2(x, y),
    })
}

#[derive(Default)]
pub(super) struct InputState {
    primary: Option<(u64, Pos2)>,
    modifiers: Modifiers,
    paste_requested: bool,
}

impl InputState {
    pub(super) fn take_raw(
        &mut self,
        queue: &mut InputQueue,
        frame: Frame,
        scale: f32,
    ) -> RawInput {
        let bounds = Rect::from_min_size(Pos2::ZERO, frame.size_pixels / scale);
        let mut raw = RawInput {
            screen_rect: Some(bounds),
            time: Some(frame.now_seconds),
            ..RawInput::default()
        };
        let viewport = raw.viewports.entry(ViewportId::ROOT).or_default();
        viewport.native_pixels_per_point = Some(frame.density);
        viewport.inner_rect = Some(bounds);
        viewport.focused = Some(true);
        if queue.reset_pointer
            && let Some((id, pos)) = self.primary.take()
        {
            push_touch(&mut raw.events, TouchPhase::Cancel, id, pos);
            self.cancel_pointer(&mut raw.events);
        }
        for event in queue.events.drain(..) {
            match event {
                InputEvent::Touch { phase, id, pixels } => {
                    self.touch(&mut raw.events, phase, id, pixels / scale)
                }
                InputEvent::Key { code, down, meta } => self.key(&mut raw.events, code, down, meta),
                InputEvent::Egui(event) => raw.events.push(event),
            }
        }
        raw
    }

    pub(super) fn take_paste_request(&mut self) -> bool {
        std::mem::take(&mut self.paste_requested)
    }

    fn touch(&mut self, events: &mut Vec<Event>, phase: TouchPhase, id: u64, pos: Pos2) {
        match phase {
            TouchPhase::Start if self.primary.is_none() => {
                self.primary = Some((id, pos));
                events.push(Event::PointerMoved(pos));
                self.button(events, pos, true);
            }
            TouchPhase::Move if self.primary.is_some_and(|(primary, _)| primary == id) => {
                self.primary = Some((id, pos));
                events.push(Event::PointerMoved(pos));
            }
            TouchPhase::End | TouchPhase::Cancel => self.finish_touch(events, phase, id, pos),
            _ => {}
        }
        push_touch(events, phase, id, pos);
    }

    fn finish_touch(&mut self, events: &mut Vec<Event>, phase: TouchPhase, id: u64, pos: Pos2) {
        if self.primary.is_none_or(|(primary, _)| primary != id) {
            return;
        }
        self.primary = None;
        if phase == TouchPhase::Cancel {
            self.cancel_pointer(events);
        } else {
            self.release_pointer(events, pos);
        }
    }

    fn cancel_pointer(&self, events: &mut Vec<Event>) {
        // Clear the pressed button without treating Android's cancellation as a click.
        let outside = egui::pos2(-10_000.0, -10_000.0);
        events.push(Event::PointerMoved(outside));
        self.release_pointer(events, outside);
    }

    fn release_pointer(&self, events: &mut Vec<Event>, pos: Pos2) {
        self.button(events, pos, false);
        events.push(Event::PointerGone);
    }

    fn button(&self, events: &mut Vec<Event>, pos: Pos2, pressed: bool) {
        events.push(Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: self.modifiers,
        });
    }

    fn key(&mut self, events: &mut Vec<Event>, code: i32, down: bool, meta: i32) {
        const SHIFT: i32 = 0x01;
        const ALT: i32 = 0x02;
        const CTRL: i32 = 0x1000;
        let ctrl = meta & CTRL != 0;
        let modifiers = Modifiers {
            alt: meta & ALT != 0,
            ctrl,
            shift: meta & SHIFT != 0,
            command: ctrl,
            ..Modifiers::NONE
        };
        if self.modifiers != modifiers {
            self.modifiers = modifiers;
            events.push(Event::ModifiersChanged(modifiers));
        }
        let Some(key) = egui_key(code) else {
            return;
        };
        if ctrl && down && self.clipboard_shortcut(events, key) {
            return;
        }
        events.push(Event::Key {
            key,
            physical_key: Some(key),
            pressed: down,
            repeat: false,
            modifiers: self.modifiers,
        });
    }

    fn clipboard_shortcut(&mut self, events: &mut Vec<Event>, key: Key) -> bool {
        match key {
            Key::C => events.push(Event::Copy),
            Key::X => events.push(Event::Cut),
            Key::V => self.paste_requested = true,
            _ => return false,
        }
        true
    }
}

fn push_touch(events: &mut Vec<Event>, phase: TouchPhase, id: u64, pos: Pos2) {
    events.push(Event::Touch {
        device_id: TouchDeviceId(0),
        id: TouchId(id),
        phase,
        pos,
        force: None,
    });
}

fn egui_key(code: i32) -> Option<Key> {
    // Android KeyEvent.KEYCODE_* values. Text, including non-Latin scripts, arrives via IME.
    Some(match code {
        19 => Key::ArrowUp,
        20 => Key::ArrowDown,
        21 => Key::ArrowLeft,
        22 => Key::ArrowRight,
        29 => Key::A,
        31 => Key::C,
        50 => Key::V,
        52 => Key::X,
        53 => Key::Y,
        54 => Key::Z,
        61 => Key::Tab,
        62 => Key::Space,
        66 | 160 => Key::Enter,
        67 => Key::Backspace,
        92 => Key::PageUp,
        93 => Key::PageDown,
        111 => Key::Escape,
        112 => Key::Delete,
        122 => Key::Home,
        123 => Key::End,
        _ => return None,
    })
}
