use std::fs::{self, DirBuilder};
use std::io;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use client_core::{ClientHandle, SubscriptionRefreshMode};
use eframe::egui;
use vpn_application::client::ClientEnginePort;
use vpn_presentation::Controller;
use vpn_presentation::session::ClientError;

use crate::catalog::AppleCatalogProtection;

pub fn open(
    context: &egui::Context,
    engine: impl FnOnce(&Path) -> Result<Box<dyn ClientEnginePort>, ClientError>,
) -> Result<Controller, ClientError> {
    let directory = data_directory()?;
    let adapter = engine(&directory)?;
    let client = ClientHandle::spawn_with_platform(
        directory,
        Box::new(AppleCatalogProtection::default()),
        adapter,
        SubscriptionRefreshMode::Automatic,
    )?;
    let dirty = Arc::new(AtomicBool::new(true));
    let signal = Arc::clone(&dirty);
    let context = context.clone();
    client.set_notifier(Arc::new(move || {
        signal.store(true, Ordering::Release);
        context.request_repaint();
    }));
    Ok(Controller::new(client, dirty))
}

fn data_directory() -> Result<PathBuf, ClientError> {
    let unavailable = ClientError::StorageUnavailable;
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or(unavailable)?;
    let base = home.join("Library/Application Support");
    fs::create_dir_all(&base).map_err(|_| unavailable)?;
    let directory = base.join("Blazer");
    match DirBuilder::new().mode(0o700).create(&directory) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(_) => return Err(unavailable),
    }
    let metadata = fs::symlink_metadata(&directory).map_err(|_| unavailable)?;
    if !metadata.is_dir() || metadata.permissions().mode() & 0o077 != 0 {
        return Err(unavailable);
    }
    directory.canonicalize().map_err(|_| unavailable)
}
