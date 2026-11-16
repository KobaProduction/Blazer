//! Use cases and ports independent of GUI, protocol implementations and OS APIs.

mod cancellation;
pub mod client;
pub mod provider;
pub mod session;
pub mod subscription_refresh;

pub const MAX_SUBSCRIPTION_BYTES: usize = 1024 * 1024;
