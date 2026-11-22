use std::io;

use client_ui::{BLAZER_ICON_PNG, BlazerView};
use eframe::egui;

pub fn run(
    title: &'static str,
    app_id: &'static str,
    open: impl FnOnce(&egui::Context) -> io::Result<BlazerView> + 'static,
) -> eframe::Result {
    let icon = eframe::icon_data::from_png_bytes(BLAZER_ICON_PNG)
        .expect("Blazer window icon must be a valid PNG");
    let viewport = egui::ViewportBuilder::default()
        .with_app_id(app_id)
        .with_icon(icon);
    #[cfg(target_os = "macos")]
    let viewport = viewport
        .with_inner_size([1014.0, 880.0])
        .with_min_inner_size([360.0, 420.0]);
    eframe::run_native(
        title,
        eframe::NativeOptions {
            viewport,
            renderer: eframe::Renderer::Wgpu,
            persist_window: false,
            ..Default::default()
        },
        Box::new(move |creation| Ok(Box::new(Window(open(&creation.egui_ctx)?)))),
    )
}

struct Window(BlazerView);

impl eframe::App for Window {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.0.ui(ui);
    }

    fn on_exit(&mut self) {
        self.0.close();
    }
}
