use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::client_engine::{render_android_tunnel, validate_profile};
use jni::jni_str;
use vpn_application::client::{
    Cancellation, ClientEnginePort, ClientError, ConnectionMode, EngineRuntime, ProxyProfile,
    RunningConnection,
};
use vpn_application::session::ModeCapabilities;
use vpn_domain::client::ProxyTransport;

use super::AndroidPlatform;

pub struct AndroidEngine {
    hooks: Arc<AndroidPlatform>,
    available: bool,
    started: bool,
}

impl AndroidEngine {
    pub fn new(hooks: Arc<AndroidPlatform>, available: bool) -> Self {
        Self {
            hooks,
            available,
            started: false,
        }
    }

    fn load_engine(&mut self, cancel: &Cancellation) -> Result<(), ClientError> {
        cancel.check()?;
        self.available = self
            .hooks
            .boolean_hook(jni_str!("loadEngine"))
            .unwrap_or(false);
        if !self.available {
            return Err(ClientError::EngineStart);
        }
        Ok(())
    }

    fn start_tunnel(
        &mut self,
        profile: &ProxyProfile,
        cancel: &Cancellation,
    ) -> Result<(), ClientError> {
        cancel.check()?;
        let reservation =
            TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).map_err(|_| ClientError::EngineStart)?;
        let readiness = reservation
            .local_addr()
            .map_err(|_| ClientError::EngineStart)?;
        let descriptor = self
            .hooks
            .open_tunnel()
            .map_err(|_| ClientError::EngineStart)?;
        if descriptor < 0 {
            return Err(ClientError::EngineStart);
        }
        let config = render_android_tunnel(profile, descriptor, readiness.port())?;
        cancel.check()?;
        drop(reservation);
        if !self
            .hooks
            .start_tunnel(&config)
            .map_err(|_| ClientError::EngineStart)?
        {
            return Err(ClientError::EngineStart);
        }
        self.started = true;
        self.await_readiness(readiness, cancel)
    }

    fn await_readiness(
        &self,
        address: SocketAddr,
        cancel: &Cancellation,
    ) -> Result<(), ClientError> {
        let deadline = Instant::now() + Duration::from_secs(12);
        while Instant::now() < deadline {
            cancel.check()?;
            if !self.is_running()? {
                return Err(ClientError::EngineStopped);
            }
            if self
                .hooks
                .boolean_hook(jni_str!("tunnelRouted"))
                .map_err(|_| ClientError::EngineReadiness)?
                && probe(address).is_ok()
            {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(30));
        }
        Err(ClientError::EngineReadiness)
    }

    fn is_running(&self) -> Result<bool, ClientError> {
        self.hooks
            .boolean_hook(jni_str!("tunnelRunning"))
            .map_err(|_| ClientError::EngineStopped)
    }
}

fn probe(address: SocketAddr) -> std::io::Result<()> {
    let timeout = Duration::from_millis(80);
    let mut stream = TcpStream::connect_timeout(&address, timeout)?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    stream.write_all(&[5, 1, 0])?;
    let mut reply = [0; 2];
    stream.read_exact(&mut reply)?;
    if reply != [5, 0] {
        return Err(std::io::ErrorKind::InvalidData.into());
    }
    Ok(())
}

impl ClientEnginePort for AndroidEngine {
    fn capabilities(&self) -> ModeCapabilities {
        ModeCapabilities {
            tun: self.available,
            ..ModeCapabilities::default()
        }
    }

    fn validate_profile(&self, profile: &ProxyProfile) -> Result<(), ClientError> {
        // Alternate placement is enabled for the verified Windows engine.
        if matches!(&profile.transport, ProxyTransport::Xhttp { metadata, .. } if !metadata.is_default())
        {
            return Err(ClientError::UnsupportedFeature);
        }
        validate_profile(profile)
    }

    fn start(
        &mut self,
        mode: ConnectionMode,
        profile: &ProxyProfile,
        _local_port: u16,
        cancel: &Cancellation,
    ) -> Result<(), ClientError> {
        if !self.available || mode != ConnectionMode::Tun {
            return Err(ClientError::UnsupportedMode);
        }
        self.validate_profile(profile)?;
        let result = self
            .load_engine(cancel)
            .and_then(|()| self.start_tunnel(profile, cancel));
        if result.is_err() {
            self.stop()?;
        }
        result
    }

    fn stop(&mut self) -> Result<(), ClientError> {
        if !self
            .hooks
            .boolean_hook(jni_str!("stopTunnel"))
            .unwrap_or(false)
        {
            return Err(ClientError::EngineStopped);
        }
        self.started = false;
        Ok(())
    }

    fn poll(&mut self) -> Result<EngineRuntime, ClientError> {
        if self.started {
            if self.is_running()? {
                return Ok(EngineRuntime::Running(RunningConnection::Tun));
            }
            self.stop()?;
        }
        Ok(EngineRuntime::Stopped)
    }
}
