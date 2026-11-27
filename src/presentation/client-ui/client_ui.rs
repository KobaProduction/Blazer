//! Shared egui view for native Blazer hosts.

mod view;

pub const BLAZER_ICON_PNG: &[u8] = include_bytes!("assets/blazer.png");

pub use view::{AppUpdateStatus, AppUpdates, BlazerView, UiHost, UpdateRelease, decode_qr_file};
#[cfg(all(debug_assertions, feature = "demo"))]
pub use view::{
    ConfigurationKind, ConfigurationPreview, Screen, SettingsPage, selector_capture_id,
};
