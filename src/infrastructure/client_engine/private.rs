use std::fs;
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
#[cfg(windows)]
use win_custody::{SecurityDescriptor, current_user_sid_string};

use vpn_domain::client::ClientError;

pub(super) struct PrivateDirectory(pub(super) PathBuf);

impl PrivateDirectory {
    pub(super) fn create(data_directory: &Path) -> Result<Self, ClientError> {
        let base = data_directory.join("runtime");
        fs::create_dir_all(&base).map_err(|_| ClientError::StorageUnavailable)?;
        reject_reparse(&base)?;
        let base = base
            .canonicalize()
            .map_err(|_| ClientError::StorageUnavailable)?;
        let path = base.join(format!("engine-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&path).map_err(|_| ClientError::StorageUnavailable)?;
        reject_reparse(&path)?;
        let owned = Self(path);
        make_private(&owned.0)?;
        Ok(owned)
    }
}

impl Drop for PrivateDirectory {
    fn drop(&mut self) {
        // Only remove known files in our uniquely created, non-reparse directory.
        if reject_reparse(&self.0).is_err() {
            return;
        }
        for filename in ["engine.exe", "profile.yaml", "wintun.dll"] {
            let _ = fs::remove_file(self.0.join(filename));
        }
        let _ = fs::remove_dir(&self.0);
    }
}

#[cfg(windows)]
fn reject_reparse(path: &Path) -> Result<(), ClientError> {
    use std::os::windows::fs::MetadataExt;
    let metadata = fs::symlink_metadata(path).map_err(|_| ClientError::StorageUnavailable)?;
    if metadata.file_attributes() & 0x400 != 0 {
        Err(ClientError::StorageUnavailable)
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn reject_reparse(path: &Path) -> Result<(), ClientError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| ClientError::StorageUnavailable)?;
    if metadata.file_type().is_symlink() {
        Err(ClientError::StorageUnavailable)
    } else {
        Ok(())
    }
}

#[cfg(windows)]
fn make_private(path: &Path) -> Result<(), ClientError> {
    // The API can also reset descendants, so only apply it to our empty directory.
    if fs::read_dir(path)
        .map_err(|_| ClientError::StorageUnavailable)?
        .next()
        .is_some()
    {
        return Err(ClientError::StorageUnavailable);
    }
    let sid = current_user_sid_string().map_err(|_| ClientError::StorageUnavailable)?;
    let mut sddl = String::from("D:P(A;OICI;FA;;;SY)");
    if sid != "S-1-5-18" {
        sddl.push_str(&format!("(A;OICI;FA;;;{sid})"));
    }
    SecurityDescriptor::from_sddl(&sddl)
        .and_then(|descriptor| descriptor.apply_to_tree(path, true))
        .map_err(|_| ClientError::StorageUnavailable)
}

#[cfg(not(windows))]
fn make_private(path: &Path) -> Result<(), ClientError> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|_| ClientError::StorageUnavailable)
}

#[cfg(windows)]
pub(super) fn hidden_command(path: &Path) -> Command {
    let mut command = Command::new(path);
    command.creation_flags(0x08000000);
    command
}

#[cfg(not(windows))]
pub(super) fn hidden_command(path: &Path) -> Command {
    Command::new(path)
}
