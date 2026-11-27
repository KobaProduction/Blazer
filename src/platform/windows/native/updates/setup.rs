use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;
use std::process::{Command, ExitStatus};

use client_ui::UpdateRelease;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

use super::feed::ReleaseAsset;

pub(super) const MAX_SETUP_BYTES: u64 = 256 * 1024 * 1024;
const STAGING_MARKER: &str = "blazer-update.json";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StagingMarker {
    product: String,
    file: String,
}

pub(super) struct StagedSetup {
    locked: File,
    directory: TempDir,
    asset: ReleaseAsset,
}

pub(super) enum SetupOutcome {
    Completed,
    Cancelled(i32),
}

impl StagedSetup {
    pub(super) fn release(&self) -> &UpdateRelease {
        &self.asset.release
    }

    pub(super) fn launch(self) -> Result<SetupOutcome, String> {
        verify(&self.locked, self.asset.size, &self.asset.sha256)
            .map_err(|error| format!("SetupVerification: {error}"))?;
        // The unelevated NSIS wizard captures the operator and owns its UAC step.
        // It closes the managed application only after installation confirmation.
        let status = Command::new(self.directory.path().join(&self.asset.name))
            .arg("/UPDATE")
            .status()
            .map_err(|error| format!("SetupLaunch: {error}"))?;
        // Both the verified file lock and staging directory outlive the wizard.
        interpret_exit(status)
    }
}

pub(super) fn stage(
    asset: ReleaseAsset,
    body: impl Read,
    on_progress: impl FnMut(u64),
) -> io::Result<StagedSetup> {
    if asset.size == 0
        || asset.size > MAX_SETUP_BYTES
        || !valid_digest(&asset.sha256)
        || !valid_name(&asset.name)
    {
        return Err(invalid("invalid setup size or digest"));
    }
    let directory = tempfile::Builder::new()
        .prefix("BlazerUpdate-")
        .tempdir_in(cache_root()?)?;
    let path = directory.path().join(&asset.name);
    let mut destination = File::create_new(&path)?;
    let body = DownloadReader {
        body,
        received: 0,
        on_progress,
    };
    let copied = io::copy(&mut body.take(asset.size + 1), &mut destination)?;
    if copied != asset.size {
        return Err(invalid("setup size does not match GitHub metadata"));
    }
    destination.sync_all()?;
    drop(destination);
    // FILE_SHARE_READ permits execution but prevents replacement/deletion between
    // digest validation and the bootstrap's elevated child loading the same file.
    let locked = OpenOptions::new().read(true).share_mode(1).open(&path)?;
    verify(&locked, asset.size, &asset.sha256)?;
    let marker = StagingMarker {
        product: "KobaProduction.Blazer.Windows.Update".into(),
        file: asset.name.clone(),
    };
    serde_json::to_writer(
        File::create_new(directory.path().join(STAGING_MARKER))?,
        &marker,
    )?;
    Ok(StagedSetup {
        directory,
        locked,
        asset,
    })
}

struct DownloadReader<R, F> {
    body: R,
    received: u64,
    on_progress: F,
}

impl<R: Read, F: FnMut(u64)> Read for DownloadReader<R, F> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let count = self.body.read(buffer)?;
        if count > 0 {
            self.received += count as u64;
            (self.on_progress)(self.received);
        }
        Ok(count)
    }
}

fn cache_root() -> io::Result<std::path::PathBuf> {
    let root = std::env::temp_dir().join("BlazerUpdates");
    fs::create_dir_all(&root)?;
    if !regular_path(&root, true) {
        return Err(invalid("update cache is not a regular directory"));
    }
    Ok(root)
}

pub(super) fn cleanup_staging() {
    let Ok(root) = cache_root() else {
        return;
    };
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.take(16).flatten() {
        let name = entry.file_name();
        if name
            .to_str()
            .is_some_and(|name| name.starts_with("BlazerUpdate-"))
        {
            cleanup_directory(&entry.path());
        }
    }
}

fn cleanup_directory(path: &Path) {
    if !regular_path(path, true) {
        return;
    }
    let marker_path = path.join(STAGING_MARKER);
    let Some(marker) = read_staging_marker(&marker_path) else {
        return;
    };
    if marker.product != "KobaProduction.Blazer.Windows.Update" || !valid_name(&marker.file) {
        return;
    }
    let Ok(entries) = fs::read_dir(path) else {
        return;
    };
    let allowed = [marker.file.as_str(), STAGING_MARKER];
    for entry in entries {
        let Ok(entry) = entry else {
            return;
        };
        if !entry
            .file_name()
            .to_str()
            .is_some_and(|name| allowed.contains(&name))
            || !regular_path(&entry.path(), false)
        {
            return;
        }
    }
    // Active executables/downloads deny deletion; retain their marker for retry.
    if fs::remove_file(path.join(&marker.file)).is_err() {
        return;
    }
    let _ = fs::remove_file(marker_path);
    let _ = fs::remove_dir(path);
}

fn read_staging_marker(path: &Path) -> Option<StagingMarker> {
    if !regular_path(path, false) {
        return None;
    }
    let mut bytes = Vec::new();
    File::open(path)
        .ok()?
        .take(1025)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() > 1024 {
        return None;
    }
    serde_json::from_slice(&bytes).ok()
}

fn regular_path(path: &Path, directory: bool) -> bool {
    path.symlink_metadata().is_ok_and(|metadata| {
        metadata.is_dir() == directory && metadata.file_attributes() & 0x400 == 0
    })
}

fn verify(file: &File, size: u64, digest: &str) -> io::Result<()> {
    use std::io::{Seek, SeekFrom};
    let mut input = file.try_clone()?;
    input.seek(SeekFrom::Start(0))?;
    let mut hasher = Sha256::new();
    let copied = io::copy(&mut input.take(size + 1), &mut hasher)?;
    if copied != size || !format!("{:x}", hasher.finalize()).eq_ignore_ascii_case(digest) {
        return Err(invalid(
            "setup SHA-256 or size does not match GitHub metadata",
        ));
    }
    Ok(())
}

pub(super) fn valid_digest(digest: &str) -> bool {
    digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn valid_name(name: &str) -> bool {
    name.len() <= 160
        && name.starts_with("Blazer-")
        && name.ends_with("-setup.exe")
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
}

fn interpret_exit(status: ExitStatus) -> Result<SetupOutcome, String> {
    match status.code() {
        Some(0) => Ok(SetupOutcome::Completed),
        Some(code @ (1223 | 1602)) => Ok(SetupOutcome::Cancelled(code)),
        Some(code) => Err(format!(
            "SetupExit: {code}; installation or recovery failed"
        )),
        None => Err("SetupExit: installer terminated without an exit code".into()),
    }
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asset(bytes: &[u8]) -> ReleaseAsset {
        ReleaseAsset {
            release: UpdateRelease {
                version: "0.1".into(),
                release_url: String::new(),
                notes: "Fixture notes".into(),
            },
            name: "Blazer-0.1-windows-x86_64-setup.exe".into(),
            download_url: String::new(),
            size: bytes.len() as u64,
            sha256: format!("{:x}", Sha256::digest(bytes)),
        }
    }

    #[test]
    fn staged_setup_rejects_changed_or_oversized_payload_and_locks_verified_bytes() {
        let bytes = b"MZ synthetic installer fixture";
        let expected = asset(bytes);
        let mut traversal = expected.clone();
        traversal.name = "../Blazer-setup.exe".into();
        assert!(stage(traversal, bytes.as_slice(), |_| {}).is_err());
        assert!(stage(expected.clone(), b"changed".as_slice(), |_| {}).is_err());
        let mut tampered = bytes.to_vec();
        tampered[2] ^= 1;
        assert!(stage(expected.clone(), tampered.as_slice(), |_| {}).is_err());
        assert!(
            stage(
                expected.clone(),
                [bytes.as_slice(), b"extra"].concat().as_slice(),
                |_| {}
            )
            .is_err()
        );
        let mut progress = Vec::new();
        let body = bytes[..2].chain(&bytes[2..]);
        let staged = stage(expected, body, |received| progress.push(received)).unwrap();
        assert_eq!(progress, [2, bytes.len() as u64]);
        assert_eq!(&*staged.release().notes, "Fixture notes");
        let path = staged.directory.path().join(&staged.asset.name);
        assert!(OpenOptions::new().write(true).open(&path).is_err());
        assert!(fs::remove_file(&path).is_err());
        verify(&staged.locked, staged.asset.size, &staged.asset.sha256).unwrap();
        drop(staged);
        assert!(!path.exists());
    }

    #[test]
    fn staging_cleanup_preserves_active_and_foreign_directories() {
        let bytes = b"MZ isolated cleanup fixture";
        let staged = stage(asset(bytes), bytes.as_slice(), |_| {}).unwrap();
        let path = staged.directory.path().to_path_buf();
        cleanup_directory(&path);
        assert!(path.join(&staged.asset.name).exists());
        fs::write(path.join("unrelated.txt"), b"preserve").unwrap();
        drop(staged.locked);
        let path = staged.directory.keep();
        cleanup_directory(&path);
        assert!(path.join("unrelated.txt").exists());
        fs::remove_file(path.join("unrelated.txt")).unwrap();
        cleanup_directory(&path);
        assert!(!path.exists());
    }

    #[test]
    fn installer_cancellation_is_distinct_from_failed_maintenance() {
        use std::os::windows::process::ExitStatusExt;
        assert!(matches!(
            interpret_exit(ExitStatus::from_raw(0)),
            Ok(SetupOutcome::Completed)
        ));
        for code in [1223, 1602] {
            assert!(
                matches!(interpret_exit(ExitStatus::from_raw(code)), Ok(SetupOutcome::Cancelled(actual)) if actual == code as i32)
            );
        }
        assert!(interpret_exit(ExitStatus::from_raw(2)).is_err());
    }
}
