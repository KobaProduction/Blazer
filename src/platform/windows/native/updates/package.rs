use std::fs::{self, File};
use std::io::{self, Read};
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use vpn_application::{APP_PREVIEW, APP_VERSION};
use zip::ZipArchive;

use super::ReleaseAsset;

mod install;
pub(crate) use install::{apply, schedule_cleanup};

pub(super) const MAX_PACKAGE_BYTES: u64 = 128 * 1024 * 1024;
const MAX_EXPANDED_BYTES: u64 = 256 * 1024 * 1024;
const MAX_ENTRIES: usize = 256;
const APP: &str = "blazer.exe";
const MANIFEST: &str = "blazer-package.json";
const ZIP_NAME: &str = "release.zip";
const WORK_PREFIX: &str = ".blazer-update-";
const REQUIRED: &[&str] = &[APP, "blazecore.exe", "LICENSE", MANIFEST];
const EXTRAS: &[&str] = &[
    "LICENSE-BLAZECORE",
    "LICENSE-NOTO-EMOJI",
    "THIRD_PARTY_NOTICES.md",
];

pub(super) struct StagedUpdate {
    installation: PathBuf,
    work: TempDir,
    asset: ReleaseAsset,
}

impl StagedUpdate {
    pub(super) fn version(&self) -> &str {
        &self.asset.version
    }

    pub(super) fn launch(self) -> io::Result<()> {
        install::launch(self)
    }
}

pub(super) fn current_installation() -> io::Result<PathBuf> {
    let exe = std::env::current_exe()?;
    if !exe
        .file_name()
        .is_some_and(|name| name.eq_ignore_ascii_case(APP))
    {
        return Err(invalid(
            "the current executable is outside a Blazer package",
        ));
    }
    let directory = exe
        .parent()
        .ok_or_else(|| invalid("missing application directory"))?;
    validate_package(directory, APP_VERSION, APP_PREVIEW)?;
    directory.canonicalize()
}

pub(super) fn stage(
    directory: &Path,
    asset: ReleaseAsset,
    body: impl Read,
) -> io::Result<StagedUpdate> {
    let parent = directory
        .parent()
        .ok_or_else(|| invalid("missing package parent"))?;
    let work = tempfile::Builder::new()
        .prefix(WORK_PREFIX)
        .tempdir_in(parent)?;
    let path = work.path().join(ZIP_NAME);
    let mut file = File::create_new(&path)?;
    let written = io::copy(&mut body.take(asset.size + 1), &mut file)?;
    if written != asset.size {
        return Err(invalid("unexpected release archive size"));
    }
    file.sync_all()?;
    drop(file);
    verify_archive(&path, asset.size, &asset.sha256)?;

    Ok(StagedUpdate {
        installation: directory.to_owned(),
        work,
        asset,
    })
}

pub(super) fn valid_digest(hash: &str) -> bool {
    hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn verify_archive(path: &Path, size: u64, hash: &str) -> io::Result<()> {
    if size == 0 || size > MAX_PACKAGE_BYTES || !valid_digest(hash) || !regular_file(path)? {
        return Err(invalid("invalid release archive identity"));
    }
    let mut file = File::open(path)?.take(size + 1);
    let mut hasher = Sha256::new();
    if io::copy(&mut file, &mut hasher)? != size
        || !format!("{:x}", hasher.finalize()).eq_ignore_ascii_case(hash)
    {
        return Err(invalid(
            "release archive SHA-256 or size does not match GitHub",
        ));
    }
    Ok(())
}

fn unpack(path: &Path, target: &Path) -> io::Result<()> {
    let mut archive = ZipArchive::new(File::open(path)?)?;
    if archive.len() > MAX_ENTRIES {
        return Err(invalid("too many files in the update package"));
    }
    let mut remaining = MAX_EXPANDED_BYTES;
    for index in 0..archive.len() {
        let item = archive.by_index(index)?;
        let relative =
            approved_entry(item.name()).ok_or_else(|| invalid("unexpected archive path"))?;
        if !item.is_file() || item.is_symlink() || item.size() > remaining {
            return Err(invalid("unsupported or oversized archive entry"));
        }
        let destination = target.join(relative);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut file = File::create_new(destination)?;
        let size = item.size();
        let written = io::copy(&mut item.take(size + 1), &mut file)?;
        if written != size {
            return Err(invalid("archive entry size mismatch"));
        }
        file.sync_all()?;
        remaining -= written;
    }
    Ok(())
}

fn approved_entry(name: &str) -> Option<&str> {
    if REQUIRED.contains(&name) || EXTRAS.contains(&name) {
        return Some(name);
    }
    let leaf = name.strip_prefix("licenses/")?;
    valid_license_name(leaf).then_some(name)
}

fn valid_license_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 100
        && bytes[0].is_ascii_alphanumeric()
        && bytes[bytes.len() - 1].is_ascii_alphanumeric()
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(byte))
        && !reserved_windows_name(name)
}

fn reserved_windows_name(name: &str) -> bool {
    let base = name
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    matches!(base.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (base.len() == 4
            && (base.starts_with("COM") || base.starts_with("LPT"))
            && base.as_bytes()[3].is_ascii_digit())
}

#[derive(Deserialize)]
struct PackageIdentity {
    version: String,
    target: String,
    channel: String,
}

fn validate_package(directory: &Path, version: &str, preview: bool) -> io::Result<()> {
    if !regular_directory(directory)? {
        return Err(invalid("expected a dedicated Windows package directory"));
    }
    for name in REQUIRED {
        if !regular_file(&directory.join(name))? {
            return Err(invalid("missing required Windows package file"));
        }
    }
    let mut metadata = Vec::new();
    File::open(directory.join(MANIFEST))?
        .take(1025)
        .read_to_end(&mut metadata)?;
    if metadata.len() > 1024 {
        return Err(invalid("oversized package manifest"));
    }
    let identity: PackageIdentity = serde_json::from_slice(&metadata)?;
    let channel = if preview { "preview" } else { "stable" };
    if identity.version != version
        || identity.target != "windows-x86_64"
        || identity.channel != channel
    {
        return Err(invalid(
            "Windows package identity does not match the release",
        ));
    }
    validate_inventory(directory)
}

fn validate_inventory(directory: &Path) -> io::Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            return Err(invalid("unrecognized package entry"));
        };
        if name == "licenses" {
            validate_licenses(&entry.path())?;
        } else if approved_entry(name).is_none() || !regular_file(&entry.path())? {
            return Err(invalid(
                "package directory contains files unrelated to Blazer",
            ));
        }
    }
    Ok(())
}

fn validate_licenses(path: &Path) -> io::Result<()> {
    if !regular_directory(path)? {
        return Err(invalid("invalid package license directory"));
    }
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        if !entry.file_name().to_str().is_some_and(valid_license_name)
            || !regular_file(&entry.path())?
        {
            return Err(invalid("invalid package license file"));
        }
    }
    Ok(())
}

fn regular_file(path: &Path) -> io::Result<bool> {
    let metadata = fs::symlink_metadata(path)?;
    Ok(metadata.is_file() && metadata.file_attributes() & 0x400 == 0)
}

fn regular_directory(path: &Path) -> io::Result<bool> {
    let metadata = fs::symlink_metadata(path)?;
    Ok(metadata.is_dir() && metadata.file_attributes() & 0x400 == 0)
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use super::*;

    fn fixture(version: &str, extra: Option<&str>) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(io::Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default();
        for name in REQUIRED.iter().copied().chain(extra) {
            writer.start_file(name, options).unwrap();
            if name == MANIFEST {
                write!(
                    writer,
                    "{{\"version\":\"{version}\",\"target\":\"windows-x86_64\",\"channel\":\"stable\"}}"
                )
                .unwrap();
            } else {
                writer.write_all(b"fixture").unwrap();
            }
        }
        writer.finish().unwrap().into_inner()
    }

    #[test]
    fn stage_rejects_tampering_and_unpacks_the_expected_package() {
        let directory = tempfile::tempdir().unwrap();
        let bytes = fixture("1.10", Some("licenses/THIRD_PARTY.txt"));
        let asset = ReleaseAsset {
            version: "1.10".into(),
            download_url: String::new(),
            size: bytes.len() as u64,
            sha256: format!("{:x}", Sha256::digest(&bytes)),
        };
        let destination = directory.path().join("client");
        let mut tampered = bytes.clone();
        tampered[0] ^= 1;
        assert!(stage(&destination, asset.clone(), tampered.as_slice()).is_err());
        let update = stage(&destination, asset, bytes.as_slice()).unwrap();
        let output = directory.path().join("new");
        fs::create_dir(&output).unwrap();
        unpack(&update.work.path().join(ZIP_NAME), &output).unwrap();
        validate_package(&output, "1.10", false).unwrap();
        assert!(validate_package(&output, "1.9", false).is_err());
        assert!(validate_package(&output, "1.10", true).is_err());
        fs::write(output.join("user-data.json"), b"leave untouched").unwrap();
        assert!(validate_package(&output, "1.10", false).is_err());
    }

    #[test]
    fn rejects_paths_outside_package_and_windows_device_names() {
        let directory = tempfile::tempdir().unwrap();
        let zip = directory.path().join(ZIP_NAME);
        fs::write(&zip, fixture("1.10", Some("licenses/../../outside.txt"))).unwrap();
        let output = directory.path().join("new");
        fs::create_dir(&output).unwrap();
        assert!(unpack(&zip, &output).is_err());
        assert!(!directory.path().join("outside.txt").exists());
        assert!(approved_entry("licenses/NUL.txt").is_none());
        assert!(approved_entry("licenses/LICENSE:stream").is_none());
    }
}
