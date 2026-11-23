use std::io::Read;
use std::net::IpAddr;
use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::Deserialize;
use vpn_application::client::Cancellation;
use vpn_domain::client::{ClientError, TunAdapterKind, TunConflict};

const QUERY: &str = include_str!("preflight.ps1");
const LIMIT: u64 = 256 * 1024;

#[derive(Deserialize, Default)]
struct Snapshot {
    adapters: Vec<Adapter>,
    routes: Vec<Route>,
    ras: Vec<RasConnection>,
}

#[derive(Deserialize)]
struct Adapter {
    index: u32,
    name: String,
    description: String,
    interface_type: u32,
    connected: bool,
    hardware: bool,
    component: String,
    driver: String,
}

#[derive(Deserialize)]
struct Route {
    index: u32,
    prefix: String,
}

#[derive(Deserialize)]
struct RasConnection {
    name: String,
    description: String,
}

pub(super) fn require_no_external_vpn(cancel: &Cancellation) -> Result<(), ClientError> {
    let snapshot = query(cancel)?;
    match conflict(&snapshot)? {
        Some(detail) => Err(ClientError::TunConflict(detail)),
        None => Ok(()),
    }
}

fn query(cancel: &Cancellation) -> Result<Snapshot, ClientError> {
    let unavailable = ClientError::TunPreflightUnavailable;
    cancel.check()?;
    let root = std::env::var_os("SystemRoot").ok_or(unavailable)?;
    let system = std::path::Path::new(&root).join("System32");
    let executable = system.join("WindowsPowerShell/v1.0/powershell.exe");
    let mut child = Command::new(executable)
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            QUERY,
        ])
        .env_clear()
        .env("SystemRoot", &root)
        .env("WINDIR", root)
        .env("PATH", &system)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .creation_flags(0x0800_0000)
        .spawn()
        .map_err(|_| unavailable)?;
    let stdout = child.stdout.take().ok_or(unavailable)?;
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout
            .take(LIMIT + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    });
    let result = wait_query(&mut child, cancel);
    if result.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    let bytes = reader
        .join()
        .map_err(|_| unavailable)?
        .map_err(|_| unavailable)?;
    result?;
    if bytes.len() as u64 > LIMIT {
        return Err(unavailable);
    }
    serde_json::from_slice(&bytes).map_err(|_| unavailable)
}

fn wait_query(child: &mut std::process::Child, cancel: &Cancellation) -> Result<(), ClientError> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        cancel.check()?;
        if let Some(status) = child
            .try_wait()
            .map_err(|_| ClientError::TunPreflightUnavailable)?
        {
            return if status.success() {
                Ok(())
            } else {
                Err(ClientError::TunPreflightUnavailable)
            };
        }
        if Instant::now() >= deadline {
            return Err(ClientError::TunPreflightUnavailable);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn conflict(snapshot: &Snapshot) -> Result<Option<TunConflict>, ClientError> {
    let routes = snapshot
        .routes
        .iter()
        .map(|route| Ok((route.index, forwarding(&route.prefix)?)))
        .collect::<Result<Vec<_>, ClientError>>()?;
    if let Some(connection) = snapshot.ras.first() {
        return Ok(Some(TunConflict::new(
            &connection.name,
            &connection.description,
            TunAdapterKind::WindowsVpn,
        )));
    }
    Ok(snapshot.adapters.iter().find_map(|adapter| {
        if !adapter.connected || adapter.hardware {
            return None;
        }
        let kind = vpn_kind(adapter)?;
        routes
            .iter()
            .any(|(index, forwarding)| *index == adapter.index && *forwarding)
            .then(|| TunConflict::new(&adapter.name, &adapter.description, kind))
    }))
}

fn vpn_kind(adapter: &Adapter) -> Option<TunAdapterKind> {
    let driver = adapter
        .driver
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let component = adapter.component.to_ascii_lowercase();
    if driver == "wireguard.sys" || component == "wireguard" {
        return Some(TunAdapterKind::WireGuard);
    }
    if matches!(
        driver.as_str(),
        "tap0901.sys" | "tap0801.sys" | "ovpn-dco.sys" | "ovpn-dco-win.sys"
    ) || component.starts_with("tap0901")
        || component.starts_with("ovpn-dco")
    {
        return Some(TunAdapterKind::OpenVpn);
    }
    if driver == "wintun.sys" || component == "wintun" {
        return Some(TunAdapterKind::Tunnel);
    }
    if adapter.interface_type == 23
        && matches!(
            component.as_str(),
            "ms_agilevpnminiport" | "ms_l2tpminiport" | "ms_pptpminiport" | "ms_sstpminiport"
        )
    {
        return Some(TunAdapterKind::WindowsVpn);
    }
    None
}

fn forwarding(prefix: &str) -> Result<bool, ClientError> {
    let invalid = ClientError::TunPreflightUnavailable;
    let (address, length) = prefix.split_once('/').ok_or(invalid)?;
    let address = address.parse::<IpAddr>().map_err(|_| invalid)?;
    let length = length.parse::<u8>().map_err(|_| invalid)?;
    if length > if address.is_ipv4() { 32 } else { 128 } {
        return Err(invalid);
    }
    Ok(match address {
        IpAddr::V4(address) => {
            !address.is_loopback()
                && !address.is_link_local()
                && !address.is_multicast()
                && !address.is_broadcast()
                && (!address.is_unspecified() || length == 0)
        }
        IpAddr::V6(address) => {
            !address.is_loopback()
                && !address.is_unicast_link_local()
                && !address.is_multicast()
                && (!address.is_unspecified() || length == 0)
        }
    })
}

#[cfg(test)]
mod tests;
