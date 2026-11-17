//! Platform-independent client entities and provider metadata policies.

pub mod client;
pub mod provider;
pub mod routing;

pub const APP_VERSION: &str = match option_env!("BLAZER_RELEASE_VERSION") {
    Some(version) => version,
    None => concat!(
        env!("CARGO_PKG_VERSION_MAJOR"),
        ".",
        env!("CARGO_PKG_VERSION_MINOR")
    ),
};
pub const APP_PREVIEW: bool = option_env!("BLAZER_PREVIEW_BUILD").is_some();
