use std::io::Read;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use client_ui::{AppUpdateStatus, AppUpdates};
use eframe::egui;
use reqwest::{StatusCode, Url, blocking::Client, redirect};
use serde::Deserialize;
use vpn_application::{APP_PREVIEW, APP_VERSION, updates::ReleaseVersion};

pub(super) mod package;

const REPOSITORY: &str = "https://github.com/KobaProduction/Blazer";
const DEFAULT_FEED: &str = "https://api.github.com/repos/KobaProduction/Blazer/releases/latest";
const MAX_FEED_BYTES: u64 = 512 * 1024;

#[derive(Default)]
enum State {
    #[default]
    Idle,
    Checking,
    Current,
    Unpublished,
    Failed,
    Available(ReleaseAsset),
    Downloading,
    Ready(package::StagedUpdate),
    Restarting,
}

impl State {
    fn status(&self) -> AppUpdateStatus {
        match self {
            Self::Idle => AppUpdateStatus::Idle,
            Self::Checking => AppUpdateStatus::Checking,
            Self::Current => AppUpdateStatus::Current,
            Self::Unpublished => AppUpdateStatus::Unpublished,
            Self::Failed => AppUpdateStatus::Failed,
            Self::Available(asset) => AppUpdateStatus::Available {
                version: asset.version.clone(),
                release_url: format!("{REPOSITORY}/releases/tag/v{}", asset.version),
            },
            Self::Ready(update) => AppUpdateStatus::Ready {
                version: update.version().to_owned(),
            },
            Self::Downloading | Self::Restarting => AppUpdateStatus::Downloading,
        }
    }

    fn busy(&self) -> bool {
        matches!(self, Self::Checking | Self::Downloading | Self::Restarting)
    }
}

struct Updater {
    state: Mutex<State>,
    endpoint: Url,
    context: egui::Context,
    installation: Option<PathBuf>,
}

pub(super) fn controls(context: &egui::Context) -> Option<AppUpdates> {
    let endpoint = feed_endpoint(option_env!("BLAZER_RELEASE_FEED_URL").unwrap_or(DEFAULT_FEED))?;
    let updater = Arc::new(Updater {
        state: Mutex::new(State::Idle),
        endpoint,
        context: context.clone(),
        installation: package::current_installation().ok(),
    });
    let for_check = Arc::clone(&updater);
    let for_install = Arc::clone(&updater);
    let for_status = Arc::clone(&updater);
    let download = updater.installation.is_some().then(|| {
        let for_download = Arc::clone(&updater);
        Arc::new(move || for_download.download()) as Arc<dyn Fn() + Send + Sync>
    });
    Some(AppUpdates {
        check: Arc::new(move || for_check.check()),
        download,
        install: Arc::new(move || for_install.install()),
        status: Arc::new(move || {
            for_status
                .state
                .lock()
                .map_or(AppUpdateStatus::Failed, |state| state.status())
        }),
    })
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
        self.run_job(|updater| check_feed(&updater.endpoint));
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
        self.run_job(move |updater| {
            let installation = updater.installation.as_deref()?;
            let response = download_response(&asset).ok()?;
            package::stage(installation, asset, response)
                .ok()
                .map(State::Ready)
        });
    }

    fn install(self: &Arc<Self>) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        if !matches!(*state, State::Ready(_)) {
            return;
        }
        let State::Ready(update) = std::mem::replace(&mut *state, State::Restarting) else {
            return;
        };
        drop(state);
        self.run_job(move |updater| {
            update.launch().ok()?;
            updater
                .context
                .send_viewport_cmd(egui::ViewportCommand::Close);
            Some(State::Restarting)
        });
    }

    fn run_job(self: &Arc<Self>, work: impl FnOnce(&Self) -> Option<State> + Send + 'static) {
        self.context.request_repaint();
        let updater = Arc::clone(self);
        if std::thread::Builder::new()
            .name("blazer-updates".into())
            .spawn(move || {
                let result = work(&updater).unwrap_or(State::Failed);
                updater.publish(result);
            })
            .is_err()
        {
            self.publish(State::Failed);
        }
    }

    fn publish(&self, status: State) {
        if let Ok(mut state) = self.state.lock() {
            *state = status;
        }
        self.context.request_repaint();
    }
}

fn feed_endpoint(value: &str) -> Option<Url> {
    let url = Url::parse(value).ok()?;
    (url.scheme() == "https"
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none())
    .then_some(url)
}

fn client() -> reqwest::blocking::ClientBuilder {
    Client::builder()
        .https_only(true)
        .user_agent(format!("Blazer/{APP_VERSION}"))
}

fn check_feed(endpoint: &Url) -> Option<State> {
    let response = client()
        .redirect(redirect::Policy::none())
        .timeout(Duration::from_secs(10))
        .build()
        .ok()?
        .get(endpoint.clone())
        .header("Accept", "application/vnd.github+json")
        .send()
        .ok()?;
    if response.status() == StatusCode::NOT_FOUND {
        return Some(State::Unpublished);
    }
    if !response.status().is_success() {
        return None;
    }
    let mut bytes = Vec::new();
    response
        .take(MAX_FEED_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() as u64 > MAX_FEED_BYTES {
        return None;
    }
    evaluate(
        serde_json::from_slice(&bytes).ok()?,
        APP_VERSION,
        APP_PREVIEW,
    )
}

fn allowed_asset_redirect(url: &Url) -> bool {
    url.scheme() == "https"
        && url.port_or_known_default() == Some(443)
        && matches!(
            url.host_str(),
            Some(
                "github.com"
                    | "release-assets.githubusercontent.com"
                    | "objects.githubusercontent.com"
            )
        )
        && url.username().is_empty()
        && url.password().is_none()
}

fn download_response(asset: &ReleaseAsset) -> reqwest::Result<reqwest::blocking::Response> {
    client()
        .redirect(redirect::Policy::custom(|attempt| {
            if attempt.previous().len() < 5 && allowed_asset_redirect(attempt.url()) {
                attempt.follow()
            } else {
                attempt.error("unexpected GitHub release asset redirect")
            }
        }))
        .timeout(Duration::from_secs(180))
        .build()?
        .get(&asset.download_url)
        .send()?
        .error_for_status()
}

#[derive(Clone)]
pub(super) struct ReleaseAsset {
    version: String,
    download_url: String,
    size: u64,
    sha256: String,
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
    size: u64,
    digest: Option<String>,
}

impl Asset {
    fn approved(self, version: &str, name: &str, download_url: &str) -> Option<ReleaseAsset> {
        let sha256 = self.digest.as_deref()?.strip_prefix("sha256:")?;
        if self.name != name
            || self.browser_download_url != download_url
            || self.size == 0
            || self.size > package::MAX_PACKAGE_BYTES
            || !package::valid_digest(sha256)
        {
            return None;
        }
        Some(ReleaseAsset {
            version: version.to_owned(),
            download_url: self.browser_download_url,
            size: self.size,
            sha256: sha256.to_owned(),
        })
    }
}

fn evaluate(release: Release, installed: &str, preview: bool) -> Option<State> {
    if release.draft || release.prerelease {
        return None;
    }
    let version = release.tag_name.strip_prefix('v')?;
    if !ReleaseVersion::parse(version)?.can_install_over(ReleaseVersion::parse(installed)?, preview)
    {
        return Some(State::Current);
    }
    let name = format!("Blazer-{version}-windows-x86_64.zip");
    let download_url = format!("{REPOSITORY}/releases/download/{}/{name}", release.tag_name);
    release
        .assets
        .into_iter()
        .find_map(|asset| asset.approved(version, &name, &download_url))
        .map(State::Available)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release() -> Release {
        Release {
            tag_name: "v1.10".into(),
            draft: false,
            prerelease: false,
            assets: vec![Asset {
                name: "Blazer-1.10-windows-x86_64.zip".into(),
                browser_download_url: format!(
                    "{REPOSITORY}/releases/download/v1.10/Blazer-1.10-windows-x86_64.zip"
                ),
                size: 1,
                digest: Some(format!("sha256:{}", "ab".repeat(32))),
            }],
        }
    }

    #[test]
    fn only_an_exact_newer_stable_windows_release_is_available() {
        assert!(
            matches!(evaluate(release(), "1.2", false), Some(State::Available(asset)) if asset.version == "1.10")
        );
        assert!(matches!(
            evaluate(release(), "1.10", false),
            Some(State::Current)
        ));
        assert!(matches!(
            evaluate(release(), "1.10", true),
            Some(State::Available(_))
        ));
        assert!(matches!(
            evaluate(release(), "2.0", false),
            Some(State::Current)
        ));
        let mut item = release();
        item.assets[0].digest = None;
        assert!(evaluate(item, "1.0", false).is_none());
        let mut item = release();
        item.assets[0].browser_download_url = "https://example.invalid/package.zip".into();
        assert!(evaluate(item, "1.0", false).is_none());
        let mut item = release();
        item.prerelease = true;
        assert!(evaluate(item, "1.0", false).is_none());
    }

    #[test]
    fn feed_and_asset_transport_validate_destinations() {
        assert!(feed_endpoint(DEFAULT_FEED).is_some());
        assert!(feed_endpoint("https://private@example.invalid/latest.json").is_none());
        assert!(feed_endpoint("http://example.invalid/latest.json").is_none());
        assert!(allowed_asset_redirect(
            &Url::parse("https://release-assets.githubusercontent.com/test").unwrap()
        ));
        assert!(!allowed_asset_redirect(
            &Url::parse("https://github.com.example.invalid/test").unwrap()
        ));
    }
}
