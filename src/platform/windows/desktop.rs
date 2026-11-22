//! Native window and application composition for Blazer.

#![cfg(any(windows, target_os = "linux"))]

mod host;
#[cfg(windows)]
mod native;
#[cfg(windows)]
mod window_state;

pub use host::ui_host;
#[cfg(windows)]
pub use native::{apply_update, run, run_after_update, run_import};

const DEFAULT_WINDOW_SIZE: [f32; 2] = [1014.0, 880.0];
const MIN_WINDOW_SIZE: [f32; 2] = [360.0, 420.0];

pub struct NativeView {
    window: client_ui::BlazerView,
    persist_ui: bool,
    #[cfg(windows)]
    placement: Option<window_state::WindowPlacement>,
}

impl NativeView {
    pub fn new(window: client_ui::BlazerView, persist_ui: bool) -> Self {
        Self {
            window,
            persist_ui,
            #[cfg(windows)]
            placement: None,
        }
    }

    #[cfg(windows)]
    fn with_window_placement(mut self, creation: &eframe::CreationContext<'_>) -> Self {
        self.placement = window_state::WindowPlacement::new(creation);
        self
    }
}

impl eframe::App for NativeView {
    fn ui(&mut self, ui: &mut eframe::egui::Ui, _frame: &mut eframe::Frame) {
        #[cfg(windows)]
        if let Some(placement) = &mut self.placement {
            placement.observe();
        }
        self.window.ui(ui);
    }

    #[cfg(windows)]
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        if let Some(placement) = &mut self.placement {
            placement.save(storage);
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.window.close();
    }

    fn persist_egui_memory(&self) -> bool {
        self.persist_ui
    }
}

pub fn native_options() -> eframe::NativeOptions {
    let icon = eframe::icon_data::from_png_bytes(client_ui::BLAZER_ICON_PNG)
        .expect("Blazer window icon must be a valid PNG");
    eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size(DEFAULT_WINDOW_SIZE)
            .with_min_inner_size(MIN_WINDOW_SIZE)
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
