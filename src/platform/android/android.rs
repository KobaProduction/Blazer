//! Android composition root and transport for the shared presentation controller.

#[cfg(target_os = "android")]
mod bridge;

#[cfg(target_os = "android")]
mod renderer;
