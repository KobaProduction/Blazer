use std::io::{Read, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use nix::sys::socket::{getsockopt, sockopt::LocalPeerPid};
use vpn_application::client::{
    Cancellation, ClientEnginePort, EngineRuntime, ProxyProfile, RunningConnection,
};
use vpn_infrastructure::client_engine::{render_apple_tunnel, validate_profile};
use vpn_presentation::session::{ClientError, ConnectionMode, ModeCapabilities, RoutingProfile};

const CONTROL_TIMEOUT: Duration = Duration::from_secs(1);

#[derive(PartialEq, Eq)]
enum TunnelState {
    Idle,
    Starting,
    Running,
    Failed,
    Stopping,
}

fn control(operation: u8, payload: &[u8]) -> Result<TunnelState, ClientError> {
    let name = format!("bz-ne.{}", std::process::id());
    let path = std::env::temp_dir().join(name);
    let mut socket = UnixStream::connect(path).map_err(|_| ClientError::EngineStopped)?;
    let peer = getsockopt(&socket, LocalPeerPid).map_err(|_| ClientError::EngineStopped)?;
    if u32::try_from(peer) != Ok(std::process::id()) {
        return Err(ClientError::EngineStopped);
    }
    socket
        .set_read_timeout(Some(CONTROL_TIMEOUT))
        .and_then(|()| socket.set_write_timeout(Some(CONTROL_TIMEOUT)))
        .and_then(|()| socket.write_all(&[operation]))
        .and_then(|()| socket.write_all(payload))
        .and_then(|()| socket.shutdown(Shutdown::Write))
        .map_err(|_| ClientError::EngineStopped)?;
    let mut status = [0];
    socket
        .read_exact(&mut status)
        .map_err(|_| ClientError::EngineStopped)?;
    match status[0] {
        0 => Ok(TunnelState::Idle),
        1 => Ok(TunnelState::Starting),
        2 => Ok(TunnelState::Running),
        3 => Ok(TunnelState::Failed),
        4 => Ok(TunnelState::Stopping),
        _ => Err(ClientError::EngineStopped),
    }
}

#[derive(Default)]
pub struct AppleTunnel {
    started: bool,
}

impl ClientEnginePort for AppleTunnel {
    fn capabilities(&self) -> ModeCapabilities {
        ModeCapabilities {
            tun: true,
            ..ModeCapabilities::default()
        }
    }

    fn validate_profile(&self, profile: &ProxyProfile) -> Result<(), ClientError> {
        validate_profile(profile)
    }

    fn start(
        &mut self,
        mode: ConnectionMode,
        profile: &ProxyProfile,
        port: u16,
        cancel: &Cancellation,
    ) -> Result<(), ClientError> {
        self.start_with_routing(mode, profile, port, None, cancel)
    }

    fn supports_routing(&self, mode: ConnectionMode) -> bool {
        mode == ConnectionMode::Tun
    }

    fn start_with_routing(
        &mut self,
        mode: ConnectionMode,
        profile: &ProxyProfile,
        _port: u16,
        routing: Option<&RoutingProfile>,
        cancel: &Cancellation,
    ) -> Result<(), ClientError> {
        if mode != ConnectionMode::Tun {
            return Err(ClientError::UnsupportedMode);
        }
        cancel.check()?;
        let configuration = render_apple_tunnel(profile, routing)?;
        if control(b'+', &configuration).map_err(|_| ClientError::EngineStart)?
            != TunnelState::Starting
        {
            return Err(ClientError::EngineStart);
        }
        self.started = true;
        if let Err(error) = wait_for_start(cancel) {
            self.stop()?;
            return Err(error);
        }
        Ok(())
    }

    fn stop(&mut self) -> Result<(), ClientError> {
        if !self.started {
            return Ok(());
        }
        control(b'-', &[])?;
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if control(b'?', &[])? == TunnelState::Idle {
                self.started = false;
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        Err(ClientError::EngineStopped)
    }

    fn poll(&mut self) -> Result<EngineRuntime, ClientError> {
        if !self.started {
            return Ok(EngineRuntime::Stopped);
        }
        match control(b'?', &[])? {
            TunnelState::Running => Ok(EngineRuntime::Running(RunningConnection::Tun)),
            TunnelState::Idle => {
                self.started = false;
                Ok(EngineRuntime::Stopped)
            }
            _ => Err(ClientError::EngineStopped),
        }
    }
}

fn wait_for_start(cancel: &Cancellation) -> Result<(), ClientError> {
    let deadline = Instant::now() + Duration::from_secs(90);
    while Instant::now() < deadline {
        cancel.check()?;
        match control(b'?', &[])? {
            TunnelState::Running => return Ok(()),
            TunnelState::Starting => std::thread::sleep(Duration::from_millis(50)),
            _ => return Err(ClientError::EngineStart),
        }
    }
    Err(ClientError::EngineReadiness)
}
