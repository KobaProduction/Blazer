use std::sync::{Arc, Mutex};

use client_ui::{AppUpdateStatus, AppUpdates};
use eframe::egui;

mod feed;
mod installation;
mod setup;

use feed::ReleaseAsset;

#[derive(Default)]
enum State {
    #[default]
    Idle,
    Checking,
    Current,
    Unpublished,
    Failed(String),
    Cancelled(i32),
    Available(ReleaseAsset),
    Downloading,
    Ready(setup::StagedSetup),
    Installing,
}

impl State {
    fn status(&self) -> AppUpdateStatus {
        match self {
            Self::Idle => AppUpdateStatus::Idle,
            Self::Checking => AppUpdateStatus::Checking,
            Self::Current => AppUpdateStatus::Current,
            Self::Unpublished => AppUpdateStatus::Unpublished,
            Self::Failed(details) => AppUpdateStatus::Failed {
                details: details.clone(),
            },
            Self::Cancelled(code) => AppUpdateStatus::Cancelled { code: *code },
            Self::Available(asset) => AppUpdateStatus::Available {
                version: asset.display_version.clone(),
                release_url: asset.release_url.clone(),
            },
            Self::Ready(update) => AppUpdateStatus::Ready {
                version: update.version().to_owned(),
            },
            Self::Downloading => AppUpdateStatus::Downloading,
            Self::Installing => AppUpdateStatus::Installing,
        }
    }

    fn busy(&self) -> bool {
        matches!(self, Self::Checking | Self::Downloading | Self::Installing)
    }
}

struct Updater {
    state: Mutex<State>,
    context: egui::Context,
}

pub(super) fn controls(context: &egui::Context) -> AppUpdates {
    let _ = std::thread::Builder::new()
        .name("blazer-update-cleanup".into())
        .spawn(setup::cleanup_staging);
    let updater = Arc::new(Updater {
        state: Mutex::new(State::Idle),
        context: context.clone(),
    });
    let for_check = Arc::clone(&updater);
    let for_install = Arc::clone(&updater);
    let for_status = Arc::clone(&updater);
    let download = installation::managed_installation().then(|| {
        let for_download = Arc::clone(&updater);
        Arc::new(move || for_download.download()) as Arc<dyn Fn() + Send + Sync>
    });
    AppUpdates {
        check: Arc::new(move || for_check.check()),
        download,
        install: Arc::new(move || for_install.install()),
        status: Arc::new(move || {
            for_status.state.lock().map_or_else(
                |_| AppUpdateStatus::Failed {
                    details: "UpdateState: lock unavailable".into(),
                },
                |state| state.status(),
            )
        }),
    }
}

impl Updater {
    fn check(self: &Arc<Self>) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        if state.busy() {
            return;
        }
        *state = State::Checking;
        drop(state);
        self.run_job(|_| match feed::check()? {
            feed::CheckResult::Current => Ok(State::Current),
            feed::CheckResult::Unpublished => Ok(State::Unpublished),
            feed::CheckResult::Available(asset) => Ok(State::Available(asset)),
        });
    }

    fn download(self: &Arc<Self>) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        let State::Available(asset) = &*state else {
            return;
        };
        let asset = asset.clone();
        *state = State::Downloading;
        drop(state);
        self.run_job(move |_| {
            if !installation::managed_installation() {
                return Err(
                    "ManagedInstallationRequired: install Blazer using the setup wizard".into(),
                );
            }
            let response = feed::download_response(&asset.download_url)?;
            setup::stage(asset, response)
                .map(State::Ready)
                .map_err(|error| format!("SetupDownload: {error}"))
        });
    }

    fn install(self: &Arc<Self>) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        if !matches!(*state, State::Ready(_)) {
            return;
        }
        let State::Ready(update) = std::mem::replace(&mut *state, State::Installing) else {
            return;
        };
        drop(state);
        self.run_job(move |_| {
            if !installation::managed_installation() {
                return Err("ManagedInstallationRequired: installed identity changed".into());
            }
            match update.launch()? {
                setup::SetupOutcome::Completed => Ok(State::Current),
                setup::SetupOutcome::Cancelled(code) => Ok(State::Cancelled(code)),
            }
        });
    }

    fn run_job(
        self: &Arc<Self>,
        work: impl FnOnce(&Self) -> Result<State, String> + Send + 'static,
    ) {
        self.context.request_repaint();
        let updater = Arc::clone(self);
        if std::thread::Builder::new()
            .name("blazer-updates".into())
            .spawn(move || {
                let result = work(&updater).unwrap_or_else(State::Failed);
                updater.publish(result);
            })
            .is_err()
        {
            self.publish(State::Failed(
                "UpdateWorker: could not start background task".into(),
            ));
        }
    }

    fn publish(&self, status: State) {
        if let Ok(mut state) = self.state.lock() {
            *state = status;
        }
        self.context.request_repaint();
    }
}
