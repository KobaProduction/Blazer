use std::sync::Arc;

use vpn_presentation::localization::Key;

pub type ImportResult = Result<Option<String>, Key>;
pub type PickImportFile = Arc<dyn Fn(&str) -> ImportResult + Send + Sync>;
pub type CaptureQrImage = Arc<dyn Fn() -> ImportResult + Send + Sync>;
pub type ScanQrCamera = Arc<dyn Fn(&str, &str, &str, &str) -> bool + Send + Sync>;
pub type OpenUpdates = Arc<dyn Fn(&str) + Send + Sync>;

#[derive(Clone, Default)]
pub enum AppUpdateStatus {
    #[default]
    Idle,
    Checking,
    Current,
    Unpublished,
    Failed {
        details: String,
    },
    Cancelled {
        code: i32,
    },
    Downloading,
    Installing,
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
    pub pick_configuration: Option<PickImportFile>,
    pub pick_qr_image: Option<PickImportFile>,
    pub capture_qr_image: Option<CaptureQrImage>,
    pub scan_qr_camera: Option<ScanQrCamera>,
    pub request_tunnel_start: Option<Arc<dyn Fn() -> bool + Send + Sync>>,
    pub app_updates: Option<AppUpdates>,
    pub open_updates: Option<OpenUpdates>,
}
