//! Composition root for the eframe renderer and the shared backend.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use vpn_backend::{BackendOptions, ClientHandle, SubscriptionRefreshMode};

use crate::native_options;
use crate::presentation::DesktopWindow;
use vpn_presentation::Controller;
use vpn_presentation::localization::{self, Key, Locale};
use vpn_presentation::model::error_key;

pub fn run() -> eframe::Result {
    run_window(None)
}

pub fn run_import(input: String) -> eframe::Result {
    run_window(Some(input))
}

fn run_window(import: Option<String>) -> eframe::Result {
    eframe::run_native(
        localization::text(Locale::Russian, Key::AppName),
        native_options(),
        Box::new(move |creation| {
            let mut window = create_window(
                creation,
                BackendOptions {
                    subscription_refresh: SubscriptionRefreshMode::Automatic,
                    ..BackendOptions::default()
                },
            )?;
            if let Some(input) = import {
                window.prefill_import(input);
            }
            Ok(Box::new(window))
        }),
    )
}

fn create_window(
    creation: &eframe::CreationContext<'_>,
    options: BackendOptions,
) -> Result<DesktopWindow, std::io::Error> {
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
    Ok(DesktopWindow::new(
        &creation.egui_ctx,
        Controller::new(client, dirty),
    ))
}
