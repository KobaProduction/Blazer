use std::fs::{self, DirBuilder};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use client_core::{ClientHandle, SubscriptionRefreshMode};
use client_ui::{BlazerView, UiHost};
use eframe::egui;
use vpn_infrastructure::client_engine::{BlazeCoreProcess, TrustedEngineSpec};
use vpn_presentation::Controller;
use vpn_presentation::localization::{self, Key, Locale};
use vpn_presentation::model::error_key;
use vpn_presentation::session::ClientError;

use crate::catalog::LinuxCatalogProtection;

#[cfg(test)]
mod tests;

pub fn run(import: Option<String>) -> eframe::Result {
    let icon = eframe::icon_data::from_png_bytes(client_ui::BLAZER_ICON_PNG)
        .expect("Blazer window icon must be a valid PNG");
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_app_id("blazer")
            .with_inner_size([1014.0, 880.0])
            .with_min_inner_size([360.0, 420.0])
            .with_icon(icon),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    eframe::run_native(
        localization::text(Locale::Russian, Key::AppName),
        options,
        Box::new(move |creation| {
            let context = &creation.egui_ctx;
            let controller = open_controller(context).map_err(|error| {
                std::io::Error::other(localization::text(Locale::Russian, error_key(error)))
            })?;
            let host = UiHost {
                device_info: Some(format!("Linux ({})", std::env::consts::ARCH)),
                ..Default::default()
            };
            let mut view = BlazerView::new(context, controller, host);
            if let Some(import) = import {
                view.prefill_import(import);
            }
            Ok(Box::new(NativeView(view)))
        }),
    )
}

struct NativeView(BlazerView);

impl eframe::App for NativeView {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.0.ui(ui);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.0.close();
    }
}

fn open_controller(context: &egui::Context) -> Result<Controller, ClientError> {
    let client = open_session(data_directory()?)?;
    let dirty = Arc::new(AtomicBool::new(true));
    let signal = Arc::clone(&dirty);
    let context = context.clone();
    client.set_notifier(Arc::new(move || {
        signal.store(true, Ordering::Release);
        context.request_repaint();
    }));
    Ok(Controller::new(client, dirty))
}

fn open_session(directory: PathBuf) -> Result<ClientHandle, ClientError> {
    let protection = LinuxCatalogProtection::new(&directory);
    let engine = BlazeCoreProcess::new(engine_spec()?, directory.clone(), 0);
    ClientHandle::spawn_with_platform(
        directory,
        Box::new(protection),
        Box::new(engine),
        SubscriptionRefreshMode::Automatic,
    )
}

fn data_directory() -> Result<PathBuf, ClientError> {
    let unavailable = ClientError::StorageUnavailable;
    let base = absolute_env_path("XDG_DATA_HOME")
        .or_else(|| absolute_env_path("HOME").map(|home| home.join(".local/share")))
        .ok_or(unavailable)?;
    fs::create_dir_all(&base).map_err(|_| unavailable)?;
    let directory = base.join("blazer");
    match DirBuilder::new().mode(0o700).create(&directory) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(_) => return Err(unavailable),
    }
    let metadata = fs::symlink_metadata(&directory).map_err(|_| unavailable)?;
    if !metadata.is_dir() || metadata.permissions().mode() & 0o077 != 0 {
        return Err(unavailable);
    }
    directory.canonicalize().map_err(|_| unavailable)
}

fn absolute_env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
}

fn engine_spec() -> Result<TrustedEngineSpec, ClientError> {
    if !cfg!(target_arch = "x86_64") {
        return Err(ClientError::UnsupportedMode);
    }
    let executable = std::env::current_exe().map_err(|_| ClientError::EngineNotFound)?;
    let source_path = executable.with_file_name("blazecore");
    // Development builds can show the UI without a packaged core. Without a
    // compile-time pin the shared launcher will reject every candidate binary.
    Ok(TrustedEngineSpec {
        source_path,
        expected_sha256: option_env!("BLAZER_LINUX_CORE_SHA256")
            .unwrap_or("")
            .to_owned(),
        expected_version: option_env!("BLAZER_LINUX_CORE_VERSION")
            .unwrap_or("")
            .to_owned(),
    })
}
