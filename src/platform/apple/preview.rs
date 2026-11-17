use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use client_ui::{BLAZER_ICON_PNG, BlazerView, UiHost};
use eframe::egui;
use vpn_presentation::Controller;
use vpn_presentation::session::{
    ClientCommand, ClientError, ClientSession, ClientSnapshot, RequestId, SubmitError,
};

pub fn run() -> eframe::Result {
    let icon = eframe::icon_data::from_png_bytes(BLAZER_ICON_PNG)
        .expect("Blazer window icon must be a valid PNG");
    let viewport = egui::ViewportBuilder::default()
        .with_app_id("blazer-apple-preview")
        .with_icon(icon);
    #[cfg(target_os = "macos")]
    let viewport = viewport
        .with_inner_size([1014.0, 880.0])
        .with_min_inner_size([360.0, 420.0]);
    eframe::run_native(
        "Blazer · Apple Preview",
        eframe::NativeOptions {
            viewport,
            renderer: eframe::Renderer::Wgpu,
            persist_window: false,
            ..Default::default()
        },
        Box::new(|creation| {
            let controller = Controller::new(PreviewSession, Arc::new(AtomicBool::new(false)));
            let mut view = BlazerView::new(&creation.egui_ctx, controller, UiHost::default());
            view.prefill_import(String::from("https://example.invalid/subscription"));
            Ok(Box::new(Preview(view)))
        }),
    )
}

struct Preview(BlazerView);

impl eframe::App for Preview {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.0.ui(ui);
    }

    fn on_exit(&mut self) {
        self.0.close();
    }
}

struct PreviewSession;

impl ClientSession for PreviewSession {
    fn snapshot(&self) -> ClientSnapshot {
        ClientSnapshot {
            busy: false,
            ..Default::default()
        }
    }

    fn send(&self, _command: ClientCommand) -> Result<RequestId, SubmitError> {
        Err(SubmitError::Rejected(ClientError::UnsupportedMode))
    }

    fn shutdown(&mut self) -> Result<(), ClientError> {
        Ok(())
    }
}
