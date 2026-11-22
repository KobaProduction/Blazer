use std::sync::Arc;

use eframe::egui::{Context, Vec2, ViewportBuilder};
use egui_winit::WindowSettings;
use winit::{dpi::LogicalSize, window::Window};

use super::{DEFAULT_WINDOW_SIZE, MIN_WINDOW_SIZE};

// eframe reads this key before creating the window. Its built-in writer is disabled
// so a minimized window cannot replace the last usable placement.
const WINDOW_KEY: &str = "window";

pub(super) fn restore_viewport(viewport: ViewportBuilder) -> ViewportBuilder {
    if viewport.inner_size.is_some_and(|size| !usable_size(size)) {
        return viewport.with_inner_size(MIN_WINDOW_SIZE);
    }
    viewport
}

fn usable_size(size: Vec2) -> bool {
    size.is_finite() && size.x >= MIN_WINDOW_SIZE[0] && size.y >= MIN_WINDOW_SIZE[1]
}

pub(super) struct WindowPlacement {
    window: Arc<Window>,
    context: Context,
    last: Option<WindowSettings>,
}

impl WindowPlacement {
    pub(super) fn new(creation: &eframe::CreationContext<'_>) -> Option<Self> {
        let window = Arc::clone(creation.winit_window()?);
        let saved: Option<WindowSettings> = creation
            .storage
            .and_then(|storage| eframe::get_value(storage, WINDOW_KEY));
        let last = saved.filter(|settings| usable_size(settings.inner_size_points()));
        if saved.is_some() && last.is_none() {
            recover_default_size(&window, creation.egui_ctx.zoom_factor());
        }
        let mut placement = Self {
            window,
            context: creation.egui_ctx.clone(),
            last,
        };
        // eframe keeps the window hidden until it paints the first frame.
        placement.record_geometry();
        Some(placement)
    }

    pub(super) fn observe(&mut self) {
        if self.window.is_visible() == Some(true) {
            self.record_geometry();
        }
    }

    pub(super) fn save(&mut self, storage: &mut dyn eframe::Storage) {
        self.observe();
        if let Some(settings) = &self.last {
            eframe::set_value(storage, WINDOW_KEY, settings);
        } else {
            storage.remove_string(WINDOW_KEY);
        }
    }

    fn record_geometry(&mut self) {
        if self.window.is_minimized() != Some(false) {
            return;
        }
        let previous = self.last.map(|settings| settings.normal_geometry());
        if previous.is_none() && (self.window.is_maximized() || self.window.fullscreen().is_some())
        {
            return;
        }
        let settings =
            WindowSettings::from_window(self.context.zoom_factor(), &self.window, previous);
        if usable_size(settings.inner_size_points()) {
            self.last = Some(settings);
        }
    }
}

fn recover_default_size(window: &Window, zoom_factor: f32) {
    if window.is_maximized() || window.fullscreen().is_some() {
        return;
    }
    let preferred = Vec2::from(DEFAULT_WINDOW_SIZE) * zoom_factor;
    let size = window.current_monitor().map_or(preferred, |monitor| {
        let max = monitor.size().to_logical::<f32>(monitor.scale_factor());
        preferred.min(eframe::egui::vec2(max.width, max.height))
    });
    let _ = window.request_inner_size(LogicalSize::new(size.x, size.y));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restores_saved_size_only_if_both_dimensions_are_usable() {
        for size in [
            [64.0, 64.0],
            [359.0, 880.0],
            [1014.0, 419.0],
            [640.0, f32::NAN],
        ] {
            let builder = ViewportBuilder::default().with_inner_size(size);
            assert_eq!(
                restore_viewport(builder).inner_size,
                Some(MIN_WINDOW_SIZE.into())
            );
        }
        let builder = ViewportBuilder::default()
            .with_inner_size([640.0, 590.0])
            .with_position([100.0, 150.0]);
        let restored = restore_viewport(builder);
        assert_eq!(restored.inner_size, Some([640.0, 590.0].into()));
        assert_eq!(restored.position, Some([100.0, 150.0].into()));
    }
}
