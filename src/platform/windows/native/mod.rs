//! Composition root for the eframe renderer and the client core.

use std::ffi::OsString;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

mod updates;

use client_core::{ClientCoreOptions, ClientHandle, SubscriptionRefreshMode};
use client_ui::BlazerView;

use crate::{NativeView, native_options, ui_host};
use vpn_presentation::Controller;
use vpn_presentation::localization::{self, Key, Locale};
use vpn_presentation::model::error_key;

pub fn run() -> eframe::Result {
    run_window(None, None)
}

pub fn run_import(input: String) -> eframe::Result {
    run_window(Some(input), None)
}

pub fn run_after_update(work: OsString) -> eframe::Result {
    run_window(None, Some(work))
}

pub fn apply_update(arguments: impl Iterator<Item = OsString>) -> std::io::Result<()> {
    updates::package::apply(arguments)
}

fn run_window(import: Option<String>, completed_update: Option<OsString>) -> eframe::Result {
    let client_options = ClientCoreOptions {
        subscription_refresh: SubscriptionRefreshMode::Automatic,
        ..ClientCoreOptions::default()
    };
    let options = eframe::NativeOptions {
        persistence_path: Some(client_options.data_dir.join("window.ron")),
        persist_window: false,
        window_builder: Some(Box::new(crate::window_state::restore_viewport)),
        ..native_options()
    };
    eframe::run_native(
        localization::text(Locale::Russian, Key::AppName),
        options,
        Box::new(move |creation| {
            crate::set_taskbar_icon(creation);
            let mut window = create_window(creation, client_options)?;
            if let Some(input) = import {
                window.prefill_import(input);
            }
            if let Some(work) = completed_update {
                updates::package::schedule_cleanup(work);
            }
            Ok(Box::new(
                NativeView::new(window, true).with_window_placement(creation),
            ))
        }),
    )
}

fn create_window(
    creation: &eframe::CreationContext<'_>,
    options: ClientCoreOptions,
) -> Result<BlazerView, std::io::Error> {
    let client = ClientHandle::spawn(options).map_err(|error| {
        std::io::Error::other(localization::text(Locale::Russian, error_key(error)))
    })?;
    let context = creation.egui_ctx.clone();
    let dirty = Arc::new(AtomicBool::new(true));
    let dirty_signal = Arc::clone(&dirty);
    client.set_notifier(Arc::new(move || {
        dirty_signal.store(true, Ordering::Release);
        context.request_repaint();
    }));
    let mut host = ui_host();
    host.app_updates = updates::controls(&creation.egui_ctx);
    Ok(BlazerView::new(
        &creation.egui_ctx,
        Controller::new(client, dirty),
        host,
    ))
}
