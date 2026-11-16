//! Implementations of the application's I/O ports.

#[cfg(target_os = "android")]
pub mod android;
pub mod client_engine;
pub mod client_http;
pub mod client_normalize;
pub mod client_probe;
pub mod client_store;
mod provider;
mod provider_store;

pub use provider::{subscription_display, subscription_usage};

pub fn new_profile_uuid() -> String {
    uuid::Uuid::new_v4().to_string()
}
