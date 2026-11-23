use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use vpn_application::{APP_PREVIEW, APP_VERSION};
use windows_registry::{Key, LOCAL_MACHINE};

const PRODUCT: &str = "KobaProduction.Blazer.Windows";

#[derive(Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Marker {
    schema: u32,
    product: String,
    target: String,
    version: String,
    channel: String,
    sequence: u32,
    commit: String,
}

pub(super) fn managed_installation() -> bool {
    validate_current().is_some()
}

fn validate_current() -> Option<()> {
    let current = std::env::current_exe().ok()?.canonicalize().ok()?;
    let key = LOCAL_MACHINE.open(r"Software\KobaProduction\Blazer").ok()?;
    let directory = PathBuf::from(key.get_string("InstallDirectory").ok()?)
        .canonicalize()
        .ok()?;
    let program_files = LOCAL_MACHINE
        .open(r"Software\Microsoft\Windows\CurrentVersion")
        .ok()?;
    let expected = PathBuf::from(program_files.get_string("ProgramFilesDir").ok()?)
        .join("Blazer")
        .canonicalize()
        .ok()?;
    if directory != expected || current != directory.join("blazer.exe") {
        return None;
    }
    let marker = read_marker(&directory)?;
    if marker != registry_marker(&key)? || !matches_current(&marker) {
        return None;
    }
    Some(())
}

fn read_marker(directory: &Path) -> Option<Marker> {
    let mut bytes = Vec::new();
    File::open(directory.join("blazer-installation.json"))
        .ok()?
        .take(16 * 1024 + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() > 16 * 1024 {
        return None;
    }
    serde_json::from_slice(&bytes).ok()
}

fn registry_marker(key: &Key) -> Option<Marker> {
    Some(Marker {
        schema: 1,
        product: key.get_string("ProductId").ok()?,
        target: "windows-x86_64".into(),
        version: key.get_string("Version").ok()?,
        channel: key.get_string("Channel").ok()?,
        sequence: key.get_u32("Sequence").ok()?,
        commit: key.get_string("Commit").ok()?,
    })
}

fn matches_current(marker: &Marker) -> bool {
    let channel = if APP_PREVIEW { "preview" } else { "stable" };
    marker.schema == 1
        && marker.product == PRODUCT
        && marker.target == "windows-x86_64"
        && marker.version == APP_VERSION
        && marker.channel == channel
        && option_env!("BLAZER_RELEASE_COMMIT").is_some_and(|commit| marker.commit == commit)
        && option_env!("BLAZER_RELEASE_SEQUENCE").and_then(|value| value.parse::<u32>().ok())
            == Some(marker.sequence)
}
