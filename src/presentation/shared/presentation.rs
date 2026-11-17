//! Toolkit-independent presentation for clients using the same session contract.

mod controller;
pub mod import;
pub mod localization;
pub mod model;

pub use controller::Controller;
pub use vpn_application::APP_VERSION;
pub use vpn_application::session;
