mod fonts;
mod import_source;
mod settings_state;
mod theme;
mod view_model;
mod widgets;

#[cfg(feature = "demo")]
pub use view_model::{ConfigurationKind, ConfigurationPreview, Screen, SettingsPage};
pub use widgets::DesktopWindow;
#[cfg(feature = "demo")]
pub use widgets::selector_capture_id;
