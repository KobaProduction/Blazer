use std::ffi::OsString;
use std::fs;
use std::io::{self, Read};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use clap::CommandFactory;
use futures_util::StreamExt;
use net_route::{Handle, Route};
use serde::Deserialize;
use tokio::runtime::{Builder, Runtime};
use tokio::task::JoinHandle;
use tun::AbstractDevice;
use tun2proxy::{ArgDns, Args, CancellationToken};
use vpn_application::client::{Cancellation, ClientEnginePort, EngineRuntime, RunningConnection};
use vpn_application::session::ModeCapabilities;
use vpn_domain::client::{ClientError, ConnectionMode, ProxyProfile, ProxyTransport};
use vpn_domain::routing::RoutingProfile;

use super::{BlazeCoreProcess, file_sha256, private::PrivateDirectory, run_bounded, wait_child};

const DNS_SERVER: Ipv4Addr = Ipv4Addr::new(1, 1, 1, 1);
const TUN_ADDRESS: Ipv4Addr = Ipv4Addr::new(10, 233, 0, 1);
const TUN_IPV6_ADDRESS: Ipv6Addr = Ipv6Addr::new(0xfd73, 0x686f, 0x6573, 0, 0, 0, 0, 1);
const MTU: u16 = 1500;

pub const BUNDLED_WINTUN_VERSION: &str = "0.14.1";
pub const APPROVED_WINTUN_SHA256: &str =
    "E5DA8447DC2C320EDC0FC52FA01885C103DE8C118481F683643CACC3220DAFCE";

struct Driver {
    path: PathBuf,
    sha256: String,
    bundled_version: Option<&'static str>,
}

impl Driver {
    fn configured() -> Option<Self> {
        Self::resolve(
            std::env::var_os("BLAZER_WINTUN_DLL"),
            std::env::var_os("BLAZER_WINTUN_SHA256"),
            std::env::current_exe().ok().as_deref(),
        )
    }

    fn resolve(
        path: Option<OsString>,
        sha256: Option<OsString>,
        executable: Option<&Path>,
    ) -> Option<Self> {
        match (path, sha256) {
            (None, None) => Self::bundled(executable?),
            (Some(path), Some(sha256)) => {
                Self::external(PathBuf::from(path), sha256.into_string().ok()?)
            }
            _ => None,
        }
    }

    fn bundled(executable: &Path) -> Option<Self> {
        if !cfg!(target_arch = "x86_64") || !executable.is_absolute() {
            return None;
        }
        let path = executable.parent()?.join("wintun.dll");
        if !path.is_file() {
            return None;
        }
        Some(Self {
            path,
            sha256: APPROVED_WINTUN_SHA256.to_owned(),
            bundled_version: Some(BUNDLED_WINTUN_VERSION),
        })
    }

    fn external(path: PathBuf, sha256: String) -> Option<Self> {
        if !path.is_absolute()
            || sha256.len() != 64
            || !sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return None;
        }
        Some(Self {
            path,
            sha256: sha256.to_ascii_uppercase(),
            bundled_version: None,
        })
    }

    fn stage(&self, data: &Path, cancel: &Cancellation) -> Result<PrivateDirectory, ClientError> {
        let metadata = fs::metadata(&self.path).map_err(|_| ClientError::EngineNotFound)?;
        if !metadata.is_file() || metadata.len() > 4 * 1024 * 1024 {
            return Err(ClientError::EngineUntrusted);
        }
        let directory = PrivateDirectory::create(data)?;
        let dll = directory.0.join("wintun.dll");
        fs::copy(&self.path, &dll).map_err(|_| ClientError::EngineNotFound)?;
        if file_sha256(&dll)? != self.sha256 {
            return Err(ClientError::EngineUntrusted);
        }
        let mut signature = powershell(
            "if ((Get-AuthenticodeSignature -LiteralPath $env:BLAZER_DRIVER_CHECK).Status -ne 'Valid') { exit 1 }",
        )?;
        signature.env("BLAZER_DRIVER_CHECK", dll);
        run_bounded(signature, cancel, ClientError::EngineUntrusted)?;
        Ok(directory)
    }
}

pub fn bundled_wintun_version() -> Option<&'static str> {
    Driver::configured()?.bundled_version
}

pub fn tun2proxy_version() -> Option<String> {
    // The relay's CLI metadata includes its Cargo version followed by build details.
    Args::command()
        .get_version()?
        .split_ascii_whitespace()
        .next()
        .map(str::to_owned)
}

pub struct WindowsEngine {
    blazecore: BlazeCoreProcess,
    driver: Option<Driver>,
    tunnel: Option<Tunnel>,
}

impl WindowsEngine {
    pub fn new(blazecore: BlazeCoreProcess) -> Self {
        Self {
            blazecore,
            driver: Driver::configured(),
            tunnel: None,
        }
    }

    fn start_tunnel(
        &mut self,
        profile: &ProxyProfile,
        cancel: &Cancellation,
    ) -> Result<(), ClientError> {
        let driver = self.driver.as_ref().ok_or(ClientError::UnsupportedMode)?;
        require_elevation(cancel)?;
        let directory = driver.stage(&self.blazecore.data_directory, cancel)?;
        let (profile, bypass) = upstream(profile, cancel)?;
        self.blazecore
            .start(ConnectionMode::LocalProxy, &profile, 0, cancel)?;
        let EngineRuntime::Running(RunningConnection::LocalProxy(proxy)) = self.blazecore.poll()?
        else {
            return Err(ClientError::EngineReadiness);
        };
        self.tunnel = Some(Tunnel::new(directory)?);
        self.tunnel
            .as_mut()
            .ok_or(ClientError::EngineStart)?
            .expose(proxy, bypass, cancel)
    }
}

impl ClientEnginePort for WindowsEngine {
    fn capabilities(&self) -> ModeCapabilities {
        ModeCapabilities {
            tun: self.driver.is_some(),
            ..self.blazecore.capabilities()
        }
    }

    fn validate_profile(&self, profile: &ProxyProfile) -> Result<(), ClientError> {
        self.blazecore.validate_profile(profile)
    }

    fn supports_local_proxy_routing(&self) -> bool {
        self.blazecore.supports_local_proxy_routing()
    }

    fn start(
        &mut self,
        mode: ConnectionMode,
        profile: &ProxyProfile,
        port: u16,
        cancel: &Cancellation,
    ) -> Result<(), ClientError> {
        if mode != ConnectionMode::Tun {
            return self.blazecore.start(mode, profile, port, cancel);
        }
        if self.tunnel.is_some() || self.blazecore.child.is_some() {
            return Err(ClientError::DisconnectFirst);
        }
        let result = self.start_tunnel(profile, cancel);
        if result.is_err() {
            self.stop()?;
        }
        result
    }

    fn stop(&mut self) -> Result<(), ClientError> {
        if let Some(tunnel) = &mut self.tunnel {
            tunnel.stop()?;
        }
        self.tunnel = None;
        self.blazecore.stop()
    }

    fn start_with_routing(
        &mut self,
        mode: ConnectionMode,
        profile: &ProxyProfile,
        local_port: u16,
        routing: Option<&RoutingProfile>,
        cancel: &Cancellation,
    ) -> Result<(), ClientError> {
        if routing.is_none() {
            return self.start(mode, profile, local_port, cancel);
        }
        if mode != ConnectionMode::LocalProxy {
            return Err(ClientError::UnsupportedMode);
        }
        self.blazecore
            .start_with_routing(mode, profile, local_port, routing, cancel)
    }

    fn poll(&mut self) -> Result<EngineRuntime, ClientError> {
        let runtime = self.blazecore.poll()?;
        let Some(tunnel) = &self.tunnel else {
            return Ok(runtime);
        };
        if matches!(runtime, EngineRuntime::Running(_)) && tunnel.running() {
            return Ok(EngineRuntime::Running(RunningConnection::Tun));
        }
        self.stop()?;
        Ok(EngineRuntime::Stopped)
    }
}

impl Drop for WindowsEngine {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

struct Tunnel {
    runtime: Runtime,
    routes: Arc<Handle>,
    owned: Vec<Route>,
    cancel: CancellationToken,
    relay: Option<JoinHandle<Result<usize, tun2proxy::Error>>>,
    route_monitor: Option<RouteMonitor>,
    directory: PrivateDirectory,
}

struct RouteMonitor {
    required: Vec<Route>,
    changed: Arc<AtomicBool>,
    listener: JoinHandle<()>,
}

impl RouteMonitor {
    fn start(
        runtime: &Runtime,
        routes: Arc<Handle>,
        required: Vec<Route>,
    ) -> Result<Self, ClientError> {
        let changed = Arc::new(AtomicBool::new(true));
        let updates = Arc::clone(&changed);
        let (registered, ready) = tokio::sync::oneshot::channel();
        let listener = runtime.spawn(async move {
            let events = routes.route_listen_stream();
            tokio::pin!(events);
            let _ = registered.send(());
            while events.next().await.is_some() {
                updates.store(true, Ordering::Release);
            }
        });
        let monitor = Self {
            required,
            changed,
            listener,
        };
        runtime
            .block_on(async { tokio::time::timeout(Duration::from_secs(3), ready).await })
            .map_err(|_| ClientError::EngineReadiness)?
            .map_err(|_| ClientError::EngineReadiness)?;
        Ok(monitor)
    }

    fn healthy(&self, snapshot: impl FnOnce() -> io::Result<Vec<Route>>) -> bool {
        !self.listener.is_finished()
            && (!self.changed.swap(false, Ordering::AcqRel)
                || snapshot().is_ok_and(|current| routes_present(self.required.iter(), &current)))
    }
}

impl Drop for RouteMonitor {
    fn drop(&mut self) {
        self.listener.abort();
    }
}

impl Tunnel {
    fn new(directory: PrivateDirectory) -> Result<Self, ClientError> {
        let runtime = Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .map_err(|_| ClientError::EngineStart)?;
        Ok(Self {
            runtime,
            directory,
            routes: Arc::new(Handle::new().map_err(|_| ClientError::EngineStart)?),
            owned: Vec::new(),
            cancel: CancellationToken::new(),
            relay: None,
            route_monitor: None,
        })
    }

    fn expose(
        &mut self,
        proxy: SocketAddr,
        bypass: Option<Route>,
        cancel: &Cancellation,
    ) -> Result<(), ClientError> {
        if let Some(bypass) = &bypass {
            self.add(bypass)?;
        }
        cancel.check()?;
        let device = self.make_device()?;
        let index = u32::try_from(device.tun_index().map_err(|_| ClientError::EngineStart)?)
            .map_err(|_| ClientError::EngineStart)?;
        configure_interface(index, cancel)?;
        let args = relay_options(proxy)?;
        self.relay =
            Some(
                self.runtime
                    .spawn(tun2proxy::run(device, MTU, args, self.cancel.clone())),
            );
        let captures = capture_routes(index);
        for route in &captures {
            cancel.check()?;
            self.add(route)?;
        }
        cancel.check()?;
        self.route_monitor = Some(RouteMonitor::start(
            &self.runtime,
            Arc::clone(&self.routes),
            captures.into_iter().chain(bypass).collect(),
        )?);
        cancel.check()?;
        if self.running() {
            Ok(())
        } else {
            Err(ClientError::EngineReadiness)
        }
    }

    fn make_device(&self) -> Result<tun::AsyncDevice, ClientError> {
        let dll = self.directory.0.join("wintun.dll");
        let mut config = tun::Configuration::default();
        // The interface name is exclusive to this session so an existing adapter is never opened.
        config
            .tun_name(format!("Blazer {}", uuid::Uuid::new_v4().simple()))
            .address(TUN_ADDRESS)
            .netmask(Ipv4Addr::new(255, 255, 255, 252))
            .mtu(MTU)
            .metric(1)
            .ring_capacity(4 * 1024 * 1024)
            .platform_config(|platform| {
                platform.wintun_file(dll.into_os_string());
            });
        let _guard = self.runtime.enter();
        tun::create_as_async(&config).map_err(|_| ClientError::EngineStart)
    }

    fn add(&mut self, route: &Route) -> Result<(), ClientError> {
        if self.runtime.block_on(self.routes.add(route)).is_err() {
            let current = self
                .runtime
                .block_on(self.routes.list())
                .map_err(|_| ClientError::EngineStart)?;
            return if current.iter().any(|entry| same_route(entry, route)) {
                Ok(())
            } else {
                Err(ClientError::EngineStart)
            };
        }
        self.owned.push(route.clone());
        Ok(())
    }

    fn running(&self) -> bool {
        self.relay.as_ref().is_some_and(|task| !task.is_finished())
            && self.route_monitor.as_ref().is_some_and(|monitor| {
                monitor.healthy(|| self.runtime.block_on(self.routes.list()))
            })
    }

    fn stop(&mut self) -> Result<(), ClientError> {
        self.route_monitor = None;
        while let Some(route) = self.owned.last() {
            if self.runtime.block_on(self.routes.delete(route)).is_err() {
                let current = self
                    .runtime
                    .block_on(self.routes.list())
                    .map_err(|_| ClientError::EngineStopped)?;
                if current.iter().any(|entry| same_route(entry, route)) {
                    return Err(ClientError::EngineStopped);
                }
            }
            self.owned.pop();
        }
        self.cancel.cancel();
        if let Some(relay) = &mut self.relay
            && self
                .runtime
                .block_on(async { tokio::time::timeout(Duration::from_secs(3), &mut *relay).await })
                .is_err()
        {
            relay.abort();
            return Err(ClientError::EngineStopped);
        }
        self.relay = None;
        Ok(())
    }
}

fn relay_options(proxy: SocketAddr) -> Result<Args, ClientError> {
    let proxy = format!("socks5://{proxy}")
        .parse()
        .map_err(|_| ClientError::EngineInvalid)?;
    let mut args = Args::default();
    args.proxy(proxy)
        .dns(ArgDns::OverTcp)
        .dns_addr(DNS_SERVER.into())
        .ipv6_enabled(true);
    Ok(args)
}

fn capture_routes(index: u32) -> [Route; 4] {
    [
        Ipv4Addr::UNSPECIFIED.into(),
        Ipv4Addr::new(128, 0, 0, 0).into(),
        Ipv6Addr::UNSPECIFIED.into(),
        Ipv6Addr::new(0x8000, 0, 0, 0, 0, 0, 0, 0).into(),
    ]
    .map(|address| Route::new(address, 1).with_ifindex(index).with_metric(0))
}

fn same_route(left: &Route, right: &Route) -> bool {
    let next_hop = |route: &Route| route.gateway.filter(|address| !address.is_unspecified());
    left.destination == right.destination
        && left.prefix == right.prefix
        && left.ifindex == right.ifindex
        && next_hop(left) == next_hop(right)
}

fn routes_present<'a>(mut required: impl Iterator<Item = &'a Route>, current: &[Route]) -> bool {
    required.all(|route| current.iter().any(|entry| same_route(entry, route)))
}

fn upstream(
    profile: &ProxyProfile,
    cancel: &Cancellation,
) -> Result<(ProxyProfile, Option<Route>), ClientError> {
    let address = resolve(&profile.host, profile.port, cancel)?;
    let bound = pin_address(profile, address);
    let bypass = if address.is_loopback() {
        None
    } else {
        best_route(address, cancel)?
    };
    Ok((bound, bypass))
}

fn pin_address(profile: &ProxyProfile, address: IpAddr) -> ProxyProfile {
    let mut bound = profile.clone();
    bound.host = address.to_string();
    if let ProxyTransport::WebSocket { host, .. } = &mut bound.transport
        && host.is_none()
        && profile.host.parse::<IpAddr>().is_err()
    {
        *host = Some(format!("{}:{}", profile.host, profile.port));
    }
    if let ProxyTransport::Xhttp { host, .. } = &mut bound.transport
        && host.is_none()
    {
        *host = Some(super::config::xhttp_authority(profile));
    }
    bound
}

fn resolve(host: &str, port: u16, cancel: &Cancellation) -> Result<IpAddr, ClientError> {
    if let Ok(address) = host.parse() {
        return Ok(address);
    }
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| ClientError::Network)?;
    let result = runtime.block_on(async {
        tokio::select! {
            () = cancel.cancelled() => Err(ClientError::Cancelled),
            result = tokio::time::timeout(Duration::from_secs(5), tokio::net::lookup_host((host, port))) => {
                result.map_err(|_| ClientError::Timeout)?.map_err(|_| ClientError::Network)?
                    .next().map(|address| address.ip()).ok_or(ClientError::Network)
            }
        }
    });
    runtime.shutdown_background();
    result
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
struct BestRoute {
    interface_index: u32,
    destination_prefix: String,
    next_hop: IpAddr,
}

fn best_route(address: IpAddr, cancel: &Cancellation) -> Result<Option<Route>, ClientError> {
    bypass_route(address, selected_route(address, cancel)?)
}

fn selected_route(address: IpAddr, cancel: &Cancellation) -> Result<BestRoute, ClientError> {
    let query = format!(
        "$ErrorActionPreference='Stop'; Find-NetRoute -RemoteIPAddress '{address}' | Where-Object DestinationPrefix | Select-Object InterfaceIndex,DestinationPrefix,NextHop | ConvertTo-Json -Compress"
    );
    let mut child = powershell(&query)?
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|_| ClientError::EngineStart)?;
    wait_child(&mut child, cancel, ClientError::EngineStart)?;
    let mut bytes = Vec::new();
    child
        .stdout
        .take()
        .ok_or(ClientError::EngineStart)?
        .take(4096)
        .read_to_end(&mut bytes)
        .map_err(|_| ClientError::EngineStart)?;
    serde_json::from_slice(&bytes).map_err(|_| ClientError::EngineStart)
}

fn powershell(script: &str) -> Result<Command, ClientError> {
    let root = PathBuf::from(std::env::var_os("SystemRoot").ok_or(ClientError::EngineStart)?);
    let system_powershell = root.join("System32/WindowsPowerShell/v1.0");
    let mut command = super::private::hidden_command(&system_powershell.join("powershell.exe"));
    command
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        // An inherited PowerShell 7 or user module path can shadow Windows system cmdlets.
        .env("PSModulePath", system_powershell.join("Modules"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    Ok(command)
}

fn configure_interface(index: u32, cancel: &Cancellation) -> Result<(), ClientError> {
    let readiness = interface_readiness_script(index, TUN_ADDRESS, TUN_IPV6_ADDRESS);
    let script = format!(
        "$ErrorActionPreference='Stop'; New-NetIPAddress -InterfaceIndex {index} -IPAddress '{TUN_IPV6_ADDRESS}' -PrefixLength 126 -PolicyStore ActiveStore | Out-Null; Set-DnsClientServerAddress -InterfaceIndex {index} -ServerAddresses '{DNS_SERVER}'; {readiness}"
    );
    run_bounded(powershell(&script)?, cancel, ClientError::EngineStart)
}

fn interface_readiness_script(index: u32, ipv4: Ipv4Addr, ipv6: Ipv6Addr) -> String {
    // A configured IPv6 address can still be tentative when New-NetIPAddress returns.
    // The caller bounds this wait and stops it on cancellation.
    format!(
        "while ($true) {{
            $usable = @(Get-NetIPAddress -InterfaceIndex {index} -PolicyStore ActiveStore -AddressState Preferred -ErrorAction SilentlyContinue | Select-Object -ExpandProperty IPAddress)
            if ($usable -contains '{ipv4}' -and $usable -contains '{ipv6}') {{ break }}
            Start-Sleep -Milliseconds 150
        }}"
    )
}

fn require_elevation(cancel: &Cancellation) -> Result<(), ClientError> {
    let script = "$principal = [Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent(); if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) { exit 1 }";
    run_bounded(powershell(script)?, cancel, ClientError::UnsupportedMode)
}

fn bypass_route(address: IpAddr, best: BestRoute) -> Result<Option<Route>, ClientError> {
    let prefix = best
        .destination_prefix
        .split_once('/')
        .and_then(|(_, prefix)| prefix.parse::<u8>().ok())
        .ok_or(ClientError::EngineStart)?;
    if prefix > 1 {
        return Ok(None);
    }
    let bits = if address.is_ipv4() { 32 } else { 128 };
    Ok(Some(
        Route::new(address, bits)
            .with_ifindex(best.interface_index)
            .with_gateway(best.next_hop),
    ))
}

#[cfg(test)]
mod tests;
