//! Desktop image picker and clipboard input. Payloads are kept in memory.

use std::fs::File;
use std::io::Read;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use eframe::egui;
use vpn_image_import::{Error, MAX_IMAGE_BYTES};
use vpn_presentation::import::payload;
use vpn_presentation::localization::Key;

type ReadResult = Result<Option<String>, Key>;

#[derive(Default)]
pub(super) struct ImportSource {
    clipboard: Option<Instant>,
    image: Option<mpsc::Receiver<ReadResult>>,
    pub screenshot_help: bool,
    pub error: Option<Key>,
}

impl ImportSource {
    pub fn busy(&self) -> bool {
        self.clipboard.is_some() || self.image.is_some()
    }

    pub fn paste(&mut self, ctx: &egui::Context) {
        self.error = None;
        self.clipboard = Some(Instant::now() + Duration::from_millis(800));
        ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaste);
        ctx.request_repaint_after(Duration::from_millis(30));
    }

    #[cfg(windows)]
    pub fn pick_file(&mut self, ctx: &egui::Context, title: &'static str) {
        self.read_image(ctx, move || {
            rfd::FileDialog::new()
                .set_title(title)
                .add_filter("PNG / JPEG / WebP", &["png", "jpg", "jpeg", "webp"])
                .pick_file()
                .map(read_file)
                .transpose()
        });
    }

    pub fn open_file(&mut self, ctx: &egui::Context, path: PathBuf) {
        self.read_image(ctx, move || read_file(path).map(Some));
    }

    #[cfg(windows)]
    pub fn capture_screen(&mut self, ctx: &egui::Context) {
        let window = ctx.clone();
        self.read_image(ctx, move || {
            std::thread::sleep(Duration::from_millis(300));
            let shot = win_screenshot::capture::capture_display();
            window.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            window.request_repaint();
            let shot = shot.map_err(|_| Key::QrScreenshotFailed)?;
            vpn_image_import::check_dimensions(shot.width as usize, shot.height as usize)
                .map_err(Error::key)?;
            let image = image::RgbaImage::from_raw(shot.width, shot.height, shot.pixels)
                .ok_or(Key::QrScreenshotFailed)?;
            let gray = image::DynamicImage::ImageRgba8(image).into_luma8();
            vpn_image_import::gray(gray.width() as usize, gray.height() as usize, gray.as_raw())
                .map(Some)
                .map_err(Error::key)
        });
        if self.image.is_some() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
        }
    }

    pub fn poll(&mut self, ctx: &egui::Context, image_only: bool) -> Option<String> {
        let result = if self.image.is_some() {
            self.poll_image()
        } else {
            self.poll_clipboard(ctx, image_only)
        };
        match result {
            Some(Ok(text)) => text,
            Some(Err(error)) => {
                self.error = Some(error);
                None
            }
            None => None,
        }
    }

    fn poll_image(&mut self) -> Option<ReadResult> {
        use mpsc::TryRecvError;
        let result = match self.image.as_ref()?.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) => Err(Key::QrUnreadableImage),
        };
        self.image = None;
        Some(result)
    }

    fn poll_clipboard(&mut self, ctx: &egui::Context, image_only: bool) -> Option<ReadResult> {
        let paste = ctx.input_mut(|input| {
            let index = input.events.iter().position(|event| {
                matches!(event, egui::Event::Paste(_) | egui::Event::PasteImage(_))
            })?;
            Some(input.events.remove(index))
        });
        if let Some(event) = paste {
            self.clipboard = None;
            return self.handle_paste(ctx, image_only, event);
        }
        if Instant::now() >= self.clipboard? {
            self.clipboard = None;
            return Some(Err(if image_only {
                Key::QrNeedImage
            } else {
                Key::ClipboardEmpty
            }));
        }
        ctx.request_repaint_after(Duration::from_millis(50));
        None
    }

    fn handle_paste(
        &mut self,
        ctx: &egui::Context,
        image_only: bool,
        event: egui::Event,
    ) -> Option<ReadResult> {
        match event {
            egui::Event::PasteImage(image) => {
                self.read_image(ctx, move || read_screenshot(&image).map(Some));
                None
            }
            egui::Event::Paste(_) if image_only => Some(Err(Key::QrNeedImage)),
            egui::Event::Paste(input) => Some(clipboard_text(input).map(Some)),
            _ => None,
        }
    }

    fn read_image(
        &mut self,
        ctx: &egui::Context,
        operation: impl FnOnce() -> ReadResult + Send + 'static,
    ) {
        self.error = None;
        let (send, receive) = mpsc::channel();
        let ctx = ctx.clone();
        match std::thread::Builder::new()
            .name("decode-qr".to_owned())
            .spawn(move || {
                let _ = send.send(operation());
                ctx.request_repaint();
            }) {
            Ok(_) => self.image = Some(receive),
            Err(_) => self.error = Some(Key::QrUnreadableImage),
        }
    }
}

fn clipboard_text(text: String) -> Result<String, Key> {
    if text.len() > vpn_presentation::import::MAX_TEXT_BYTES {
        return Err(Key::ClipboardTextTooLarge);
    }
    if payload(&text).is_empty() {
        return Err(Key::ClipboardEmpty);
    }
    Ok(text)
}

fn read_file(path: PathBuf) -> Result<String, Key> {
    let file = File::open(path).map_err(|_| Key::QrUnreadableImage)?;
    let mut data = Vec::new();
    file.take(MAX_IMAGE_BYTES as u64 + 1)
        .read_to_end(&mut data)
        .map_err(|_| Key::QrUnreadableImage)?;
    vpn_image_import::image(&data).map_err(Error::key)
}

fn read_screenshot(image: &egui::ColorImage) -> Result<String, Key> {
    vpn_image_import::check_dimensions(image.size[0], image.size[1]).map_err(Error::key)?;
    let gray: Vec<u8> = image
        .pixels
        .iter()
        .map(|pixel| {
            let [red, green, blue, _] = pixel.to_array().map(u16::from);
            ((77 * red + 150 * green + 29 * blue) / 256) as u8
        })
        .collect();
    vpn_image_import::gray(image.size[0], image.size[1], &gray).map_err(Error::key)
}
