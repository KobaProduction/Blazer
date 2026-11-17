use client_ui::UiHost;

pub fn ui_host() -> UiHost {
    #[cfg(windows)]
    {
        windows::ui_host()
    }
    #[cfg(not(windows))]
    {
        UiHost {
            device_info: Some(format!(
                "{} ({})",
                std::env::consts::OS,
                std::env::consts::ARCH
            )),
            ..Default::default()
        }
    }
}

#[cfg(windows)]
mod windows {
    use std::fmt::Write as _;
    use std::sync::Arc;

    use client_ui::{UiHost, decode_qr_file};
    use vpn_image_import::Error;
    use vpn_presentation::localization::Key;

    pub(super) fn ui_host() -> UiHost {
        let arch = std::env::consts::ARCH;
        let device = windows_version().map_or_else(
            || format!("Windows ({arch})"),
            |version| format!("Windows {version} ({arch})"),
        );
        UiHost {
            device_info: Some(device),
            reports_wintun: true,
            pick_qr_image: Some(Arc::new(|title| {
                rfd::FileDialog::new()
                    .set_title(title)
                    .add_filter("PNG / JPEG / WebP", &["png", "jpg", "jpeg", "webp"])
                    .pick_file()
                    .map(decode_qr_file)
                    .transpose()
            })),
            capture_qr_image: Some(Arc::new(|| capture_qr_image().map(Some))),
            request_tunnel_start: None,
            app_updates: None,
        }
    }

    fn capture_qr_image() -> Result<String, Key> {
        let shot =
            win_screenshot::capture::capture_display().map_err(|_| Key::QrScreenshotFailed)?;
        vpn_image_import::check_dimensions(shot.width as usize, shot.height as usize)
            .map_err(Error::key)?;
        let image = image::RgbaImage::from_raw(shot.width, shot.height, shot.pixels)
            .ok_or(Key::QrScreenshotFailed)?;
        let gray = image::DynamicImage::ImageRgba8(image).into_luma8();
        vpn_image_import::gray(gray.width() as usize, gray.height() as usize, gray.as_raw())
            .map_err(Error::key)
    }

    fn windows_version() -> Option<String> {
        let key = windows_registry::LOCAL_MACHINE
            .open(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion")
            .ok()?;
        let major = key.get_u32("CurrentMajorVersionNumber").ok()?;
        let minor = key.get_u32("CurrentMinorVersionNumber").ok()?;
        let build = key
            .get_string("CurrentBuildNumber")
            .ok()?
            .parse::<u32>()
            .ok()?;
        let mut version = format!("{major}.{minor}.{build}");
        if let Ok(revision) = key.get_u32("UBR") {
            let _ = write!(version, ".{revision}");
        }
        Some(version)
    }
}
