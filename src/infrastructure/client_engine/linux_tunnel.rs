use std::fs;
use std::os::unix::fs::FileTypeExt;
use std::path::Path;

use vpn_application::client::{Cancellation, RunningConnection};
use vpn_domain::client::{ClientError, ProxyProfile};
use vpn_domain::routing::RoutingProfile;
use zeroize::Zeroizing;

use super::{BlazeCoreProcess, managed_tunnel};

pub(super) fn start(
    engine: &mut BlazeCoreProcess,
    profile: &ProxyProfile,
    routing: Option<&RoutingProfile>,
    cancel: &Cancellation,
) -> Result<(), ClientError> {
    cancel.check()?;
    if !available() {
        return Err(ClientError::LinuxTunUnavailable);
    }
    let document = Zeroizing::new(managed_tunnel::render(profile, routing)?);
    let directory = engine.prepare_profile(profile, &document, cancel)?;
    engine.start_config(directory, RunningConnection::Tun, cancel)
}

fn available() -> bool {
    let can_open_tun =
        fs::metadata("/dev/net/tun").is_ok_and(|entry| entry.file_type().is_char_device());
    let has_nftables = ["/usr/sbin/nft", "/usr/bin/nft", "/sbin/nft", "/bin/nft"]
        .iter()
        .any(|path| Path::new(path).is_file());
    can_open_tun && has_nftables && can_configure_network()
}

fn can_configure_network() -> bool {
    let Ok(status) = fs::read_to_string("/proc/self/status") else {
        return false;
    };
    // The child and its nft helper need the capability after execve.
    let inherited = if rustix::process::geteuid().is_root() {
        "CapBnd:"
    } else {
        "CapAmb:"
    };
    contains_net_admin(&status, "CapEff:") && contains_net_admin(&status, inherited)
}

fn contains_net_admin(status: &str, field: &str) -> bool {
    const CAP_NET_ADMIN: u64 = 1 << 12;
    status
        .lines()
        .find_map(|line| line.strip_prefix(field))
        .and_then(|mask| u64::from_str_radix(mask.trim(), 16).ok())
        .is_some_and(|mask| mask & CAP_NET_ADMIN != 0)
}
