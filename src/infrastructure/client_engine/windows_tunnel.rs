use vpn_application::client::{Cancellation, RunningConnection};
use vpn_domain::client::{ClientError, ProxyProfile};
use vpn_domain::routing::RoutingProfile;
use windows_service::{
    service::{ServiceAccess, ServiceState},
    service_manager::{ServiceManager, ServiceManagerAccess},
};
use zeroize::Zeroizing;

use super::{BlazeCoreProcess, managed_tunnel};
mod preflight;

pub(super) fn start(
    engine: &mut BlazeCoreProcess,
    profile: &ProxyProfile,
    routing: Option<&RoutingProfile>,
    cancel: &Cancellation,
) -> Result<(), ClientError> {
    cancel.check()?;
    preflight::require_no_external_vpn(cancel)?;
    require_running_service()?;
    let document = Zeroizing::new(managed_tunnel::render(profile, routing)?);
    let directory = engine.prepare_profile(profile, &document, cancel)?;
    engine.start_config(directory, RunningConnection::Tun, cancel)
}

fn require_running_service() -> Result<(), ClientError> {
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
        .map_err(|_| ClientError::TunServiceUnavailable)?;
    let service = manager
        .open_service("BlazeCoreTun", ServiceAccess::QUERY_STATUS)
        .map_err(|_| ClientError::TunServiceUnavailable)?;
    let status = service
        .query_status()
        .map_err(|_| ClientError::TunServiceUnavailable)?;
    if status.current_state != ServiceState::Running {
        return Err(ClientError::TunServiceUnavailable);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
