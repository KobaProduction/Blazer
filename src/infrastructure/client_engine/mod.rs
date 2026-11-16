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

mod config;
mod private;
mod routing;
mod runtime_process;
mod vless_encryption;
#[cfg(windows)]
mod windows_tunnel;
#[cfg(windows)]
pub use windows_tunnel::{WindowsEngine, bundled_wintun_version, tun2proxy_version};

use self::runtime_process::RuntimeProcess;

#[cfg(target_os = "android")]
pub use config::render_android_tunnel;

pub(crate) fn validate_profile(profile: &ProxyProfile) -> Result<(), ClientError> {
    config::validate(profile)
}

#[cfg(all(test, windows))]
mod import_tests;

pub const APPROVED_ENGINE_SHA256: &str =
    "950CB4FF73B588DB817ED9DE383669377102529D75C90C7F3BB580910027B192";
pub const APPROVED_ENGINE_VERSION: &str = "0.3.7+gcce97a2c4c1c.dirty";
pub const APPROVED_ENGINE_REVISION: &str = "cce97a2c4c1c3bd0a07807f37a7caf3876c79110";

#[derive(Clone)]
pub struct TrustedEngineSpec {
    pub source_path: PathBuf,
    pub expected_sha256: String,
    pub expected_version: String,
}

pub struct ShoesProcess {
    spec: TrustedEngineSpec,
    data_directory: PathBuf,
    child: Option<RuntimeProcess>,
    private_directory: Option<private::PrivateDirectory>,
    local_address: Option<SocketAddr>,
    port_override: u16,
}

impl ShoesProcess {
    pub fn new(spec: TrustedEngineSpec, data_directory: PathBuf, port_override: u16) -> Self {
        Self {
            spec,
            data_directory,
            child: None,
            private_directory: None,
            local_address: None,
            port_override,
        }
    }

    fn prepare(
        &self,
        profile: &ProxyProfile,
        port: u16,
        routing: Option<&RoutingProfile>,
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
        let executable = directory.0.join("engine.exe");
        fs::copy(&self.spec.source_path, &executable).map_err(|_| ClientError::EngineNotFound)?;
        if file_sha256(&executable)? != self.spec.expected_sha256.to_ascii_uppercase() {
            return Err(ClientError::EngineUntrusted);
        }
        cancel.check()?;
        verify_version(
            &executable,
            &directory.0,
            &self.spec.expected_version,
            cancel,
        )?;
        let document = Zeroizing::new(config::render(profile, port, routing)?);
        let path = directory.0.join("profile.yaml");
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|_| ClientError::StorageWrite)?;
        file.write_all(&document)
            .and_then(|()| file.sync_all())
            .map_err(|_| ClientError::StorageWrite)?;
        let mut check = engine_command(&executable, &directory.0);
        check.arg("check").arg(&path);
        run_bounded(check, cancel, ClientError::EngineInvalid)?;
        Ok(directory)
    }
}

impl ClientEnginePort for ShoesProcess {
    fn capabilities(&self) -> vpn_application::session::ModeCapabilities {
        vpn_application::session::ModeCapabilities {
            local_proxy: true,
            local_http_proxy: true,
            system_proxy: false,
            tun: false,
        }
    }

    fn validate_profile(&self, profile: &ProxyProfile) -> Result<(), ClientError> {
        validate_profile(profile)
    }

    fn supports_local_proxy_routing(&self) -> bool {
        true
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
        if mode != ConnectionMode::LocalProxy {
            return Err(ClientError::UnsupportedMode);
        }
        if self.child.is_some() {
            return Err(ClientError::DisconnectFirst);
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
        let directory = self.prepare(profile, address.port(), routing, cancel)?;
        cancel.check()?;
        let profile_path = directory.0.join("profile.yaml");
        drop(reservation);
        let (child, stdout) = RuntimeProcess::spawn(&directory.0, &profile_path)
            .map_err(|_| ClientError::EngineStart)?;
        self.child = Some(child);
        self.private_directory = Some(directory);
        let result = watch_readiness(stdout).and_then(|marker| {
            wait_for_socks(
                self.child.as_mut().ok_or(ClientError::EngineStart)?,
                address,
                cancel,
                marker,
            )
        });
        // Windows Shoes has loaded the configuration by the readiness marker and only
        // Unix supports explicit signal-based reloads when file watching is disabled.
        #[cfg(windows)]
        let result = result
            .and_then(|()| fs::remove_file(profile_path).map_err(|_| ClientError::StorageWrite));
        if let Err(error) = result {
            self.stop()?;
            return Err(error);
        }
        self.local_address = Some(address);
        Ok(())
    }

    fn stop(&mut self) -> Result<(), ClientError> {
        if let Some(child) = self.child.as_mut() {
            child.stop()?;
        }
        self.child = None;
        self.local_address = None;
        self.private_directory = None;
        Ok(())
    }

    fn poll(&mut self) -> Result<EngineRuntime, ClientError> {
        let Some(child) = self.child.as_mut() else {
            return Ok(EngineRuntime::Stopped);
        };
        if !child.is_running().map_err(|_| ClientError::EngineStopped)? {
            self.child = None;
            self.local_address = None;
            self.private_directory = None;
            return Ok(EngineRuntime::Stopped);
        }
        self.local_address
            .map(|address| EngineRuntime::Running(RunningConnection::LocalProxy(address)))
            .ok_or(ClientError::EngineReadiness)
    }
}

impl Drop for ShoesProcess {
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
    let actual = output
        .trim()
        .strip_prefix("shoes ")
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
        .name("vpn-shoes-readiness".to_owned())
        .spawn(move || {
            let mut stream = BufReader::new(stdout);
            for line in Read::by_ref(&mut stream)
                .take(8192)
                .lines()
                .map_while(Result::ok)
            {
                // This exact marker is emitted by the pinned Shoes after it binds its servers.
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

fn wait_for_socks(
    child: &mut RuntimeProcess,
    address: SocketAddr,
    cancel: &Cancellation,
    marker: Receiver<()>,
) -> Result<(), ClientError> {
    let deadline = Instant::now() + Duration::from_secs(8);
    let mut owned_listener_ready = false;
    loop {
        cancel.check()?;
        if !child.is_running().map_err(|_| ClientError::EngineStart)? {
            return Err(ClientError::EngineStart);
        }
        let wait = deadline
            .saturating_duration_since(Instant::now())
            .min(Duration::from_millis(25));
        if wait.is_zero() {
            return Err(ClientError::EngineReadiness);
        }
        if !owned_listener_ready {
            match marker.recv_timeout(wait) {
                Ok(()) => owned_listener_ready = true,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => return Err(ClientError::EngineStart),
            }
        }
        if owned_listener_ready && socks_ready(address) {
            cancel.check()?;
            if !child.is_running().map_err(|_| ClientError::EngineStart)? {
                return Err(ClientError::EngineStart);
            }
            return Ok(());
        }
        std::thread::sleep(wait);
    }
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
