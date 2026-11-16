//! Shared native desktop presentation and the regular client composition root.

#[cfg(feature = "desktop")]
mod native;
#[cfg(any(feature = "desktop", feature = "demo"))]
mod presentation;

#[cfg(feature = "desktop")]
pub use native::{run, run_import};
#[cfg(feature = "demo")]
pub use presentation::{
    ConfigurationKind, ConfigurationPreview, DesktopWindow, Screen, SettingsPage,
    selector_capture_id,
};
pub use vpn_presentation::localization as i18n;

#[cfg(any(feature = "desktop", feature = "demo"))]
pub fn native_options() -> eframe::NativeOptions {
    let icon = eframe::icon_data::from_png_bytes(include_bytes!("assets/blazer.png"))
        .expect("Blazer window icon must be a valid PNG");
    eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1014.0, 880.0])
            .with_min_inner_size([700.0, 520.0])
            .with_icon(icon),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    }
}
