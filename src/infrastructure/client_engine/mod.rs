use std::fs::{self, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};
use vpn_application::client::{Cancellation, ClientEnginePort, EngineRuntime, RunningConnection};
use vpn_domain::client::{ClientError, ConnectionMode, ProxyProfile};
use vpn_domain::routing::RoutingProfile;
#[cfg(windows)]
use wait_timeout::ChildExt;
use zeroize::Zeroizing;

#[cfg(any(target_os = "android", test))]
mod android_profile;
#[cfg(any(target_os = "ios", target_os = "macos", test))]
mod apple_tunnel;
mod config;
mod graph;
mod least_ping;
#[cfg(target_os = "linux")]
mod linux_tunnel;
#[cfg(any(target_os = "linux", windows))]
mod managed_tunnel;
mod private;
#[cfg(not(target_os = "android"))]
mod profile_probe;
mod routing;
mod runtime_process;
#[cfg(windows)]
mod system_proxy;
mod vless_encryption;
#[cfg(windows)]
mod windows_tunnel;

pub use least_ping::{
    Monitor as ObservatoryMonitor, Prepared as PreparedObservatory, SelectorControl,
    SelectorControlFuture,
};
#[cfg(windows)]
pub use system_proxy::{recover_system_proxy, run_system_proxy_guard};

use self::runtime_process::RuntimeProcess;

#[cfg(target_os = "android")]
pub use android_profile::validate_android_profile;
#[cfg(any(target_os = "ios", target_os = "macos"))]
pub use apple_tunnel::render_apple_tunnel;
#[cfg(target_os = "android")]
pub use config::{render_android_tunnel, render_android_tunnel_records};

pub fn validate_profile(profile: &ProxyProfile) -> Result<(), ClientError> {
    config::validate(profile)
}

#[cfg(all(test, windows))]
mod import_tests;

pub const APPROVED_ENGINE_SHA256: &str =
    "CCA53DEC21570B841D2CBC9F414410CA5CB84F1209865296B31DCB89837E5DCF";
pub const APPROVED_ENGINE_VERSION: &str = "0.3.7+gb90121c0406c";
pub const APPROVED_ENGINE_REVISION: &str = "b90121c0406c406bb8eeee90e8405bdb9a508c59";

#[derive(Clone)]
pub struct TrustedEngineSpec {
    pub source_path: PathBuf,
    pub expected_sha256: String,
    pub expected_version: String,
}

pub struct BlazeCoreProcess {
    spec: TrustedEngineSpec,
    data_directory: PathBuf,
    child: Option<RuntimeProcess>,
    private_directory: Option<private::PrivateDirectory>,
    connection: Option<RunningConnection>,
    port_override: u16,
    #[cfg(not(target_os = "android"))]
    pending_observer: Option<least_ping::Prepared>,
    #[cfg(not(target_os = "android"))]
    observer: Option<least_ping::Monitor>,
    #[cfg(windows)]
    system_proxy: Option<system_proxy::SystemProxyGuard>,
}

impl BlazeCoreProcess {
    pub fn new(spec: TrustedEngineSpec, data_directory: PathBuf, port_override: u16) -> Self {
        Self {
            spec,
            data_directory,
            child: None,
            private_directory: None,
            connection: None,
            port_override,
            #[cfg(not(target_os = "android"))]
            pending_observer: None,
            #[cfg(not(target_os = "android"))]
            observer: None,
            #[cfg(windows)]
            system_proxy: None,
        }
    }

    fn prepare_profile(
        &mut self,
        profile: &ProxyProfile,
        document: &[u8],
        cancel: &Cancellation,
    ) -> Result<private::PrivateDirectory, ClientError> {
        #[cfg(not(target_os = "android"))]
        if let Some(prepared) = least_ping::Prepared::for_profile(profile)? {
            let mut records: Vec<serde_json::Value> =
                serde_json::from_slice(document).map_err(|_| ClientError::EngineInvalid)?;
            prepared.decorate(&mut records)?;
            let document = Zeroizing::new(
                serde_json::to_vec(&records).map_err(|_| ClientError::EngineInvalid)?,
            );
            let directory = self.prepare(&document, cancel)?;
            self.pending_observer = Some(prepared);
            return Ok(directory);
        }
        let _ = profile;
        self.prepare(document, cancel)
    }

    fn prepare(
        &self,
        document: &[u8],
        cancel: &Cancellation,
    ) -> Result<private::PrivateDirectory, ClientError> {
        if !self.spec.source_path.is_absolute() {
            return Err(ClientError::EngineUntrusted);
        }
        let metadata =
            fs::metadata(&self.spec.source_path).map_err(|_| ClientError::EngineNotFound)?;
        if !metadata.is_file() || metadata.len() > 128 * 1024 * 1024 {
            return Err(ClientError::EngineUntrusted);
        }
        cancel.check()?;
        let directory = private::PrivateDirectory::create(&self.data_directory)?;
        let executable = directory.0.join(private::RUNTIME_EXECUTABLE);
        fs::copy(&self.spec.source_path, &executable).map_err(|_| ClientError::EngineNotFound)?;
        if file_sha256(&executable)? != self.spec.expected_sha256.to_ascii_uppercase() {
            return Err(ClientError::EngineUntrusted);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&executable, fs::Permissions::from_mode(0o700))
                .map_err(|_| ClientError::EngineUntrusted)?;
        }
        cancel.check()?;
        verify_version(
            &executable,
            &directory.0,
            &self.spec.expected_version,
            cancel,
        )?;
        let path = directory.0.join("profile.yaml");
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|_| ClientError::StorageWrite)?;
        file.write_all(document)
            .and_then(|()| file.sync_all())
            .map_err(|_| ClientError::StorageWrite)?;
        let mut check = engine_command(&executable, &directory.0);
        check.arg("check").arg(&path);
        run_bounded(check, cancel, ClientError::EngineInvalid)?;
        Ok(directory)
    }

    fn start_config(
        &mut self,
        directory: private::PrivateDirectory,
        connection: RunningConnection,
        cancel: &Cancellation,
    ) -> Result<(), ClientError> {
        #[cfg(target_os = "android")]
        cancel.check()?;
        #[cfg(not(target_os = "android"))]
        if let Err(error) = cancel.check() {
            self.pending_observer = None;
            return Err(error);
        }
        let profile_path = directory.0.join("profile.yaml");
        #[cfg(not(target_os = "android"))]
        let mut observer = self.pending_observer.take();
        #[cfg(not(target_os = "android"))]
        if let Some(prepared) = observer.as_mut() {
            prepared.release_ports();
        }
        let (child, stdout) = RuntimeProcess::spawn(&directory.0, &profile_path)
            .map_err(|_| ClientError::EngineStart)?;
        self.child = Some(child);
        self.private_directory = Some(directory);
        let result = watch_readiness(stdout).and_then(|marker| {
            wait_until_ready(
                self.child.as_mut().ok_or(ClientError::EngineStart)?,
                connection,
                cancel,
                marker,
            )
        });
        // The pinned engine has finished loading the config and reload is disabled.
        let result = result
            .and_then(|()| fs::remove_file(profile_path).map_err(|_| ClientError::StorageWrite));
        if let Err(error) = result {
            self.stop()?;
            return Err(error);
        }
        self.connection = Some(connection);
        #[cfg(not(target_os = "android"))]
        if let Some(prepared) = observer {
            match prepared.start(cancel) {
                Ok(monitor) => self.observer = Some(monitor),
                Err(error) => {
                    self.stop()?;
                    return Err(error);
                }
            }
        }
        Ok(())
    }
}

impl ClientEnginePort for BlazeCoreProcess {
    fn capabilities(&self) -> vpn_application::session::ModeCapabilities {
        vpn_application::session::ModeCapabilities {
            local_proxy: true,
            local_http_proxy: true,
            system_proxy: cfg!(windows),
            tun: cfg!(any(target_os = "linux", windows)),
        }
    }

    fn validate_profile(&self, profile: &ProxyProfile) -> Result<(), ClientError> {
        validate_profile(profile)
    }

    #[cfg(not(target_os = "android"))]
    fn supports_tunnel_probe(&self) -> bool {
        true
    }

    fn supports_active_tunnel_probe(&self) -> bool {
        cfg!(windows)
    }

    #[cfg(not(target_os = "android"))]
    fn probe_tunnel(
        &self,
        profile: &ProxyProfile,
        active: Option<&ProxyProfile>,
        reuse_active_proxy: bool,
        settings: &vpn_domain::client::ProbeSettings,
        cancel: &Cancellation,
    ) -> Result<Duration, ClientError> {
        self.probe_tunnel_with_settings(profile, active, reuse_active_proxy, settings, cancel)
    }

    fn supports_routing(&self, mode: ConnectionMode) -> bool {
        self.capabilities().supports(mode)
    }

    fn start(
        &mut self,
        mode: ConnectionMode,
        profile: &ProxyProfile,
        local_port: u16,
        cancel: &Cancellation,
    ) -> Result<(), ClientError> {
        self.start_with_routing(mode, profile, local_port, None, cancel)
    }

    fn start_with_routing(
        &mut self,
        mode: ConnectionMode,
        profile: &ProxyProfile,
        local_port: u16,
        routing: Option<&RoutingProfile>,
        cancel: &Cancellation,
    ) -> Result<(), ClientError> {
        if self.child.is_some() {
            return Err(ClientError::DisconnectFirst);
        }
        #[cfg(windows)]
        if mode == ConnectionMode::Tun {
            return windows_tunnel::start(self, profile, routing, cancel);
        }
        #[cfg(target_os = "linux")]
        if mode == ConnectionMode::Tun {
            return linux_tunnel::start(self, profile, routing, cancel);
        }
        if mode != ConnectionMode::LocalProxy
            && !(cfg!(windows) && mode == ConnectionMode::SystemProxy)
        {
            return Err(ClientError::UnsupportedMode);
        }
        let configured_port = if self.port_override != 0 {
            self.port_override
        } else {
            local_port
        };
        let reservation = TcpListener::bind((Ipv4Addr::LOCALHOST, configured_port))
            .map_err(|_| ClientError::EngineStart)?;
        let address = reservation
            .local_addr()
            .map_err(|_| ClientError::EngineStart)?;
        let document = Zeroizing::new(config::render(profile, address.port(), routing)?);
        let directory = self.prepare_profile(profile, &document, cancel)?;
        drop(reservation);
        self.start_config(directory, RunningConnection::LocalProxy(address), cancel)?;
        #[cfg(windows)]
        if mode == ConnectionMode::SystemProxy {
            match system_proxy::SystemProxyGuard::start(address, cancel) {
                Ok(guard) => {
                    self.system_proxy = Some(guard);
                    self.connection = Some(RunningConnection::SystemProxy(address));
                }
                Err(error) => {
                    self.stop()?;
                    return Err(error);
                }
            }
        }
        Ok(())
    }

    fn stop(&mut self) -> Result<(), ClientError> {
        #[cfg(not(target_os = "android"))]
        {
            if let Some(observer) = self.observer.as_mut() {
                observer.stop()?;
            }
            self.observer = None;
            self.pending_observer = None;
        }
        #[cfg(windows)]
        if let Some(guard) = self.system_proxy.as_mut() {
            guard.stop()?;
            self.system_proxy = None;
        }
        if let Some(child) = self.child.as_mut() {
            child.stop()?;
        }
        self.child = None;
        self.connection = None;
        self.private_directory = None;
        Ok(())
    }

    fn poll(&mut self) -> Result<EngineRuntime, ClientError> {
        #[cfg(not(target_os = "android"))]
        if let Some(observer) = self.observer.as_ref()
            && observer.is_finished()
        {
            let error = observer.failure();
            self.stop()?;
            return Err(error);
        }
        #[cfg(windows)]
        if let Some(guard) = self.system_proxy.as_mut()
            && !guard.is_running()?
        {
            self.stop()?;
            return Err(ClientError::SystemProxyRestoreFailed);
        }
        let Some(child) = self.child.as_mut() else {
            return Ok(EngineRuntime::Stopped);
        };
        if !child.is_running().map_err(|_| ClientError::EngineStopped)? {
            self.stop()?;
            return Ok(EngineRuntime::Stopped);
        }
        self.connection
            .map(EngineRuntime::Running)
            .ok_or(ClientError::EngineReadiness)
    }
}

impl Drop for BlazeCoreProcess {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

fn engine_command(executable: &Path, directory: &Path) -> std::process::Command {
    let mut command = private::hidden_command(executable);
    command
        .current_dir(directory)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    if let Some(system_root) = std::env::var_os("SystemRoot") {
        let path = PathBuf::from(&system_root).join("System32");
        command
            .env_clear()
            .env("SystemRoot", system_root)
            .env("PATH", path);
    }
    #[cfg(unix)]
    command.env_clear();
    command
}

fn file_sha256(path: &Path) -> Result<String, ClientError> {
    let mut file = fs::File::open(path).map_err(|_| ClientError::EngineUntrusted)?;
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 16384];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|_| ClientError::EngineUntrusted)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(format!("{:X}", digest.finalize()))
}

fn verify_version(
    executable: &Path,
    directory: &Path,
    expected: &str,
    cancel: &Cancellation,
) -> Result<(), ClientError> {
    let mut command = engine_command(executable, directory);
    command.arg("-V").stdout(Stdio::piped());
    let mut child = command.spawn().map_err(|_| ClientError::EngineUntrusted)?;
    wait_child(&mut child, cancel, ClientError::EngineUntrusted)?;
    let mut output = String::new();
    child
        .stdout
        .take()
        .ok_or(ClientError::EngineUntrusted)?
        .take(4096)
        .read_to_string(&mut output)
        .map_err(|_| ClientError::EngineUntrusted)?;
    let version_line = output.trim();
    let actual = version_line
        .strip_prefix("BlazeCore ")
        .ok_or(ClientError::EngineUntrusted)?;
    if actual != expected && actual.split_once('+').map(|(version, _)| version) != Some(expected) {
        return Err(ClientError::EngineUntrusted);
    }
    Ok(())
}

fn run_bounded(
    mut command: std::process::Command,
    cancel: &Cancellation,
    error: ClientError,
) -> Result<(), ClientError> {
    let mut child = command.spawn().map_err(|_| error)?;
    wait_child(&mut child, cancel, error)
}

fn wait_child(
    child: &mut Child,
    cancel: &Cancellation,
    error: ClientError,
) -> Result<(), ClientError> {
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        if let Err(error) = cancel.check() {
            stop_child(child)?;
            return Err(error);
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        match wait_for_exit(child, remaining.min(Duration::from_millis(20))) {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(_)) => return Err(error),
            Ok(None) if Instant::now() < deadline => {}
            _ => {
                stop_child(child)?;
                return Err(error);
            }
        }
    }
}

fn wait_for_exit(child: &mut Child, timeout: Duration) -> io::Result<Option<ExitStatus>> {
    #[cfg(windows)]
    {
        child.wait_timeout(timeout)
    }
    #[cfg(not(windows))]
    {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        std::thread::sleep(timeout);
        child.try_wait()
    }
}

fn stop_child(child: &mut Child) -> Result<(), ClientError> {
    if child.try_wait().is_ok_and(|status| status.is_some()) {
        return Ok(());
    }
    let _ = child.kill();
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if child.try_wait().is_ok_and(|status| status.is_some()) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(ClientError::EngineStopped);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn watch_readiness(stdout: impl Read + Send + 'static) -> Result<Receiver<()>, ClientError> {
    let (ready, receiver) = mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("vpn-blazecore-readiness".to_owned())
        .spawn(move || {
            let mut stream = BufReader::new(stdout);
            for line in Read::by_ref(&mut stream)
                .take(8192)
                .lines()
                .map_while(Result::ok)
            {
                // This exact marker is emitted by the pinned BlazeCore after it binds its servers.
                if line == "Servers ready" {
                    let _ = ready.send(());
                    break;
                }
            }
            drop(ready);
            let _ = io::copy(&mut stream, &mut io::sink());
        })
        .map_err(|_| ClientError::EngineStart)?;
    Ok(receiver)
}

fn wait_until_ready(
    child: &mut RuntimeProcess,
    connection: RunningConnection,
    cancel: &Cancellation,
    marker: Receiver<()>,
) -> Result<(), ClientError> {
    let timeout = match connection {
        RunningConnection::Tun => Duration::from_secs(20),
        _ => Duration::from_secs(8),
    };
    let deadline = Instant::now() + timeout;
    let mut servers_ready = false;
    loop {
        check_starting_process(child, cancel)?;
        let wait = deadline
            .saturating_duration_since(Instant::now())
            .min(Duration::from_millis(25));
        if wait.is_zero() {
            return Err(ClientError::EngineReadiness);
        }
        if !servers_ready {
            match marker.recv_timeout(wait) {
                Ok(()) => servers_ready = true,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => return Err(ClientError::EngineStart),
            }
        }
        let ready = match connection {
            RunningConnection::LocalProxy(address) | RunningConnection::SystemProxy(address) => {
                socks_ready(address)
            }
            RunningConnection::Tun => true,
        };
        if ready {
            return check_starting_process(child, cancel);
        }
        std::thread::sleep(wait);
    }
}

fn check_starting_process(
    child: &mut RuntimeProcess,
    cancel: &Cancellation,
) -> Result<(), ClientError> {
    cancel.check()?;
    if !child.is_running().map_err(|_| ClientError::EngineStart)? {
        return Err(ClientError::EngineStart);
    }
    Ok(())
}

fn socks_ready(address: SocketAddr) -> bool {
    fn probe(address: SocketAddr) -> std::io::Result<bool> {
        let mut stream = TcpStream::connect_timeout(&address, Duration::from_millis(100))?;
        stream.set_read_timeout(Some(Duration::from_millis(150)))?;
        stream.set_write_timeout(Some(Duration::from_millis(150)))?;
        stream.write_all(&[5, 1, 0])?;
        let mut reply = [0u8; 2];
        stream.read_exact(&mut reply)?;
        Ok(reply == [5, 0])
    }
    probe(SocketAddr::new(
        IpAddr::V4(Ipv4Addr::LOCALHOST),
        address.port(),
    ))
    .unwrap_or(false)
}
