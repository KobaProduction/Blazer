use std::sync::Arc;

use vpn_presentation::localization::Key;

pub type ImportResult = Result<Option<String>, Key>;
pub type PickQrImage = Arc<dyn Fn(&str) -> ImportResult + Send + Sync>;
pub type CaptureQrImage = Arc<dyn Fn() -> ImportResult + Send + Sync>;

#[derive(Clone, Default)]
pub enum AppUpdateStatus {
    #[default]
    Idle,
    Checking,
    Current,
    Unpublished,
    Failed,
    Downloading,
    Ready {
        version: String,
    },
    Available {
        version: String,
        release_url: String,
    },
}

pub struct AppUpdates {
    pub check: Arc<dyn Fn() + Send + Sync>,
    pub download: Option<Arc<dyn Fn() + Send + Sync>>,
    pub install: Arc<dyn Fn() + Send + Sync>,
    pub status: Arc<dyn Fn() -> AppUpdateStatus + Send + Sync>,
}

#[derive(Default)]
pub struct UiHost {
    pub device_info: Option<String>,
    pub reports_wintun: bool,
    pub pick_qr_image: Option<PickQrImage>,
    pub capture_qr_image: Option<CaptureQrImage>,
    pub request_tunnel_start: Option<Arc<dyn Fn() -> bool + Send + Sync>>,
    pub app_updates: Option<AppUpdates>,
}
