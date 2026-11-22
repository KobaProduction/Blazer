//! A separate instance of Blazer owns Windows proxy settings until its control
//! pipe closes. It is deliberately outside the engine Job Object, so a killed
//! client cannot leave Windows pointing at a terminated local proxy.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::SocketAddr;
use std::process::{Child, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use vpn_application::client::Cancellation;
use vpn_domain::client::ClientError;
use win_custody::{MutexAcquisition, NamedMutex, SecurityDescriptor, current_user_sid_string};

#[path = "system_proxy_settings.rs"]
mod settings;

pub(super) struct SystemProxyGuard {
    child: Child,
}

impl SystemProxyGuard {
    pub(super) fn start(address: SocketAddr, cancel: &Cancellation) -> Result<Self, ClientError> {
        cancel.check()?;
        let executable =
            std::env::current_exe().map_err(|_| ClientError::SystemProxyUnavailable)?;
        let mut child = super::private::hidden_command(&executable)
            .arg("--system-proxy-guard")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| ClientError::SystemProxyUnavailable)?;
        let stdout = child
            .stdout
            .take()
            .ok_or(ClientError::SystemProxyUnavailable)?;
        let mut guard = Self { child };
        let ready = read_ready(stdout)?;
        writeln!(
            guard
                .child
                .stdin
                .as_mut()
                .ok_or(ClientError::SystemProxyUnavailable)?,
            "{address}"
        )
        .map_err(|_| ClientError::SystemProxyUnavailable)?;
        if let Err(error) = wait_ready(ready, cancel) {
            guard.stop()?;
            return Err(error);
        }
        Ok(guard)
    }

    pub(super) fn is_running(&mut self) -> Result<bool, ClientError> {
        self.child
            .try_wait()
            .map(|status| status.is_none())
            .map_err(|_| ClientError::SystemProxyUnavailable)
    }

    pub(super) fn stop(&mut self) -> Result<(), ClientError> {
        self.child.stdin.take();
        let deadline = Instant::now() + Duration::from_secs(4);
        loop {
            match self.child.try_wait() {
                Ok(Some(status)) if status.success() => return Ok(()),
                Ok(Some(_)) => return recover_system_proxy(),
                Err(_) => return Err(ClientError::SystemProxyRestoreFailed),
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(20))
                }
                Ok(None) => return Err(ClientError::SystemProxyRestoreFailed),
            }
        }
    }
}

impl Drop for SystemProxyGuard {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

fn read_ready(stdout: impl Read + Send + 'static) -> Result<Receiver<bool>, ClientError> {
    let (sender, receiver) = mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("blazer-system-proxy-ready".into())
        .spawn(move || {
            let mut line = String::new();
            let ready =
                BufReader::new(stdout).take(32).read_line(&mut line).is_ok() && line == "ready\n";
            let _ = sender.send(ready);
        })
        .map_err(|_| ClientError::SystemProxyUnavailable)?;
    Ok(receiver)
}

fn wait_ready(receiver: Receiver<bool>, cancel: &Cancellation) -> Result<(), ClientError> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        cancel.check()?;
        match receiver.recv_timeout(Duration::from_millis(20)) {
            Ok(true) => return Ok(()),
            Err(mpsc::RecvTimeoutError::Timeout) if Instant::now() < deadline => {}
            _ => return Err(ClientError::SystemProxyUnavailable),
        }
    }
}

/// Internal Windows entry point. The only input is a numeric loopback address;
/// configuration documents and subscription secrets never enter this process.
pub fn run_system_proxy_guard() -> Result<(), ClientError> {
    let mut input = io::stdin().lock();
    let command = read_command(&mut input)?;
    if command == "recover" {
        return match acquire_ownership()? {
            Some(_ownership) => settings::recover(),
            None => Ok(()),
        };
    }
    let address = parse_address(&command)?;
    let _ownership = acquire_ownership()?.ok_or(ClientError::SystemProxyUnavailable)?;
    let mut lease = settings::ProxyLease::apply(address)?;
    let result = announce_ready().and_then(|()| {
        let mut byte = [0u8; 1];
        input
            .read(&mut byte)
            .map(|_| ())
            .map_err(|_| ClientError::SystemProxyUnavailable)
    });
    lease.restore()?;
    result
}

fn read_command(input: &mut impl BufRead) -> Result<String, ClientError> {
    let mut line = String::new();
    input
        .take(128)
        .read_line(&mut line)
        .map_err(|_| ClientError::SystemProxyUnavailable)?;
    Ok(line.trim().to_owned())
}

fn parse_address(command: &str) -> Result<SocketAddr, ClientError> {
    let address: SocketAddr = command
        .parse()
        .map_err(|_| ClientError::SystemProxyUnavailable)?;
    if address.ip() != std::net::Ipv4Addr::LOCALHOST || address.port() == 0 {
        return Err(ClientError::SystemProxyUnavailable);
    }
    Ok(address)
}

/// Recover a durable lease abandoned by a terminated guardian. An active
/// guardian holds the mutex and is left untouched, including from another UI.
pub fn recover_system_proxy() -> Result<(), ClientError> {
    let executable = std::env::current_exe().map_err(|_| ClientError::SystemProxyRestoreFailed)?;
    let mut child = super::private::hidden_command(&executable)
        .arg("--system-proxy-guard")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| ClientError::SystemProxyRestoreFailed)?;
    if let Some(mut input) = child.stdin.take() {
        input
            .write_all(b"recover\n")
            .map_err(|_| ClientError::SystemProxyRestoreFailed)?;
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(_)) | Err(_) => return Err(ClientError::SystemProxyRestoreFailed),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            Ok(None) => return Err(ClientError::SystemProxyRestoreFailed),
        }
    }
}

fn announce_ready() -> Result<(), ClientError> {
    let mut stdout = io::stdout().lock();
    stdout
        .write_all(b"ready\n")
        .and_then(|()| stdout.flush())
        .map_err(|_| ClientError::SystemProxyUnavailable)
}

fn acquire_ownership() -> Result<Option<NamedMutex>, ClientError> {
    let sid = current_user_sid_string().map_err(|_| ClientError::SystemProxyUnavailable)?;
    let descriptor = SecurityDescriptor::from_sddl(&format!("D:P(A;;GA;;;SY)(A;;GA;;;{sid})"))
        .map_err(|_| ClientError::SystemProxyUnavailable)?;
    let mutex = NamedMutex::create(&format!("Global\\Blazer.SystemProxy.{sid}"), &descriptor)
        .map_err(|_| ClientError::SystemProxyUnavailable)?;
    match mutex
        .acquire(Duration::ZERO)
        .map_err(|_| ClientError::SystemProxyUnavailable)?
    {
        MutexAcquisition::Acquired | MutexAcquisition::Abandoned => Ok(Some(mutex)),
        MutexAcquisition::TimedOut => Ok(None),
    }
}
