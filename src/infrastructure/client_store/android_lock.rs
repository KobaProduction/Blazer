use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rustix::fs::{FlockOperation, fcntl_lock};
use vpn_domain::client::ClientError;

// Android's std::fs::File does not implement try_lock. POSIX locks are scoped
// to the process, so guard against a second open in the same process first.
static LOCKED_DIRECTORIES: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

pub(super) struct CatalogLock {
    file: Option<File>,
    directory: PathBuf,
}

impl CatalogLock {
    pub(super) fn acquire(directory: &Path) -> Result<Self, ClientError> {
        let directory = directory
            .canonicalize()
            .map_err(|_| ClientError::StorageUnavailable)?;
        let mut locked = LOCKED_DIRECTORIES
            .lock()
            .map_err(|_| ClientError::StorageUnavailable)?;
        if locked.contains(&directory) {
            return Err(ClientError::StorageUnavailable);
        }
        let file = super::open_lock_file(&directory)?;
        fcntl_lock(&file, FlockOperation::NonBlockingLockExclusive)
            .map_err(|_| ClientError::StorageUnavailable)?;
        locked.push(directory.clone());
        Ok(Self {
            file: Some(file),
            directory,
        })
    }
}

impl Drop for CatalogLock {
    fn drop(&mut self) {
        // Release the OS lock before allowing another open in this process.
        drop(self.file.take());
        if let Ok(mut locked) = LOCKED_DIRECTORIES.lock() {
            locked.retain(|directory| directory != &self.directory);
        }
    }
}
