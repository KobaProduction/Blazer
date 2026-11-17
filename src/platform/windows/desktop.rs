//! Native window and application composition for Blazer.

#![cfg(any(windows, target_os = "linux"))]

mod host;
#[cfg(windows)]
mod native;

pub use host::ui_host;
#[cfg(windows)]
pub use native::{apply_update, run, run_after_update, run_import};

pub struct NativeView(pub client_ui::BlazerView);

impl eframe::App for NativeView {
    fn ui(&mut self, ui: &mut eframe::egui::Ui, _frame: &mut eframe::Frame) {
        self.0.ui(ui);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.0.close();
    }
}

pub fn native_options() -> eframe::NativeOptions {
    let icon = eframe::icon_data::from_png_bytes(client_ui::BLAZER_ICON_PNG)
        .expect("Blazer window icon must be a valid PNG");
    eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1014.0, 880.0])
            .with_min_inner_size([360.0, 420.0])
            .with_app_id("blazer")
            .with_icon(icon),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    }
}

pub fn set_taskbar_icon(_creation: &eframe::CreationContext<'_>) {
    #[cfg(all(windows, target_env = "msvc"))]
    {
        use winit::{
            platform::windows::{IconExtWindows, WindowExtWindows},
            window::Icon,
        };

        // blazer.rc embeds the application icon as resource 1.
        if let Some(window) = _creation.winit_window()
            && let Ok(icon) = Icon::from_resource(1, None)
        {
            window.set_taskbar_icon(Some(icon));
        }
    }
}
