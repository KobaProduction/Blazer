use std::ffi::{OsStr, OsString};
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use vpn_application::{APP_PREVIEW, APP_VERSION, updates::ReleaseVersion};

use super::{
    APP, StagedUpdate, WORK_PREFIX, ZIP_NAME, current_installation, invalid, regular_directory,
    regular_file, unpack, validate_package, verify_archive,
};

const HELPER: &str = "apply.exe";
const HANDOFF: &str = "handoff.lock";
const FINISHED: &str = "completed";
static PARENT_HANDOFF: Mutex<Option<File>> = Mutex::new(None);

pub(super) fn launch(update: StagedUpdate) -> io::Result<()> {
    let StagedUpdate {
        installation,
        work,
        asset,
    } = update;
    let name = installation
        .file_name()
        .ok_or_else(|| invalid("missing installation name"))?;
    let mut parent = PARENT_HANDOFF
        .lock()
        .map_err(|_| invalid("cannot lock update handoff"))?;
    if parent.is_some() {
        return Err(invalid("an update is already being applied"));
    }
    let helper = work.path().join(HELPER);
    fs::copy(installation.join(APP), &helper)?;
    let handoff = open_lock(&work.path().join(HANDOFF), true)?;
    handoff.try_lock()?;
    Command::new(&helper)
        .current_dir(work.path())
        .arg("--apply-update")
        .arg(name)
        .args([asset.version, asset.size.to_string(), asset.sha256])
        .spawn()?;
    *parent = Some(handoff);
    let _ = work.keep();
    Ok(())
}

fn open_lock(path: &Path, create: bool) -> io::Result<File> {
    OpenOptions::new()
        .write(true)
        .create(create)
        .truncate(false)
        .open(path)
}

fn wait_for_lock(file: &File) -> io::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(45);
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(()),
            Err(TryLockError::WouldBlock) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(error) => return Err(error.into()),
        }
    }
}

pub(crate) fn apply(arguments: impl Iterator<Item = OsString>) -> io::Result<()> {
    let arguments: Vec<_> = arguments.collect();
    let [name, version, size, sha256] = arguments.as_slice() else {
        return Err(invalid("invalid update arguments"));
    };
    let version = argument_text(version)?;
    let installed =
        ReleaseVersion::parse(APP_VERSION).ok_or_else(|| invalid("invalid installed version"))?;
    let target = ReleaseVersion::parse(version).ok_or_else(|| invalid("invalid update version"))?;
    if !target.can_install_over(installed, APP_PREVIEW) {
        return Err(invalid(
            "update version must be newer than the installed version",
        ));
    }
    let size = argument_text(size)?
        .parse()
        .map_err(|_| invalid("invalid archive size"))?;
    let helper = std::env::current_exe()?;
    let (work, installation) = helper_paths(&helper, name)?;
    let handoff = open_lock(&work.join(HANDOFF), false)?;
    wait_for_lock(&handoff)?;
    let parent = installation
        .parent()
        .ok_or_else(|| invalid("missing installation parent"))?;
    let writer = open_lock(&parent.join(".blazer-install.lock"), true)?;
    writer.try_lock()?;

    let result = apply_package(work, &installation, version, size, argument_text(sha256)?);
    if result.is_err() && regular_file(&installation.join(APP)).unwrap_or(false) {
        let _ = Command::new(installation.join(APP))
            .current_dir(&installation)
            .spawn();
    }
    result
}

fn argument_text(value: &OsStr) -> io::Result<&str> {
    value
        .to_str()
        .ok_or_else(|| invalid("invalid update argument encoding"))
}

fn helper_paths<'a>(helper: &'a Path, name: &OsStr) -> io::Result<(&'a Path, PathBuf)> {
    let work = helper
        .parent()
        .ok_or_else(|| invalid("missing update directory"))?;
    if helper.file_name() != Some(OsStr::new(HELPER))
        || !is_work_name(work.file_name())
        || !regular_directory(work)?
    {
        return Err(invalid("the installer must run from its update directory"));
    }
    if !normal_name(name) || is_work_name(Some(name)) {
        return Err(invalid("invalid installation directory name"));
    }
    let parent = work
        .parent()
        .ok_or_else(|| invalid("missing update parent"))?;
    Ok((work, parent.join(name)))
}

fn normal_name(name: &OsStr) -> bool {
    let mut components = Path::new(name).components();
    matches!(components.next(), Some(Component::Normal(leaf)) if leaf == name)
        && components.next().is_none()
}

fn is_work_name(name: Option<&OsStr>) -> bool {
    name.and_then(OsStr::to_str).is_some_and(|name| {
        name.strip_prefix(WORK_PREFIX).is_some_and(|suffix| {
            !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_alphanumeric())
        })
    })
}

fn apply_package(
    work: &Path,
    installation: &Path,
    version: &str,
    size: u64,
    sha256: &str,
) -> io::Result<()> {
    validate_package(installation, APP_VERSION, APP_PREVIEW)?;
    let archive = work.join(ZIP_NAME);
    verify_archive(&archive, size, sha256)?;
    let payload = work.join("payload");
    fs::create_dir(&payload)?;
    unpack(&archive, &payload)?;
    validate_package(&payload, version, false)?;
    File::create_new(work.join(FINISHED))?;
    let work_name = work
        .file_name()
        .ok_or_else(|| invalid("missing update directory name"))?;
    replace_package(
        installation,
        &payload,
        &work.join("previous"),
        |directory| {
            Command::new(directory.join(APP))
                .current_dir(directory)
                .arg("--finish-update")
                .arg(work_name)
                .spawn()?;
            Ok(())
        },
    )
}

fn replace_package(
    installation: &Path,
    payload: &Path,
    previous: &Path,
    restart: impl FnOnce(&Path) -> io::Result<()>,
) -> io::Result<()> {
    fs::rename(installation, previous)?;
    if let Err(error) = fs::rename(payload, installation) {
        return restore(previous, installation, error);
    }
    if let Err(error) = restart(installation) {
        fs::rename(installation, payload).map_err(|failure| recovery_error(previous, failure))?;
        return restore(previous, installation, error);
    }
    Ok(())
}

fn restore(previous: &Path, installation: &Path, error: io::Error) -> io::Result<()> {
    fs::rename(previous, installation).map_err(|failure| recovery_error(previous, failure))?;
    Err(error)
}

fn recovery_error(previous: &Path, error: io::Error) -> io::Error {
    io::Error::other(format!(
        "Could not restore the previous package at {}: {error}",
        previous.display()
    ))
}

pub(crate) fn schedule_cleanup(name: OsString) {
    if !is_work_name(Some(&name)) {
        return;
    }
    let Ok(installation) = current_installation() else {
        return;
    };
    let Some(parent) = installation.parent() else {
        return;
    };
    let work = parent.join(name);
    if regular_directory(&work).unwrap_or(false)
        && regular_file(&work.join(FINISHED)).unwrap_or(false)
    {
        let _ = std::thread::Builder::new()
            .name("blazer-update-cleanup".into())
            .spawn(move || cleanup(work));
    }
}

fn cleanup(work: PathBuf) {
    let Ok(handoff) = open_lock(&work.join(HANDOFF), false) else {
        return;
    };
    if wait_for_lock(&handoff).is_err() {
        return;
    }
    drop(handoff);
    for _ in 0..15 {
        if fs::remove_dir_all(&work).is_ok() || !work.exists() {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directory_transaction_restores_on_restart_failure_and_commits_on_success() {
        let work = tempfile::tempdir().unwrap();
        let installation = work.path().join("client");
        let payload = work.path().join("payload");
        let previous = work.path().join("previous");
        fs::create_dir(&installation).unwrap();
        fs::create_dir(&payload).unwrap();
        fs::write(installation.join(APP), b"previous executable").unwrap();
        fs::write(payload.join(APP), b"new executable").unwrap();

        let failed = replace_package(&installation, &payload, &previous, |_| {
            Err(invalid("restart failed"))
        });
        assert!(failed.is_err());
        assert!(!previous.exists());
        assert_eq!(
            fs::read(installation.join(APP)).unwrap(),
            b"previous executable"
        );

        replace_package(&installation, &payload, &previous, |path| {
            assert_eq!(fs::read(path.join(APP))?, b"new executable");
            Ok(())
        })
        .unwrap();
        assert_eq!(
            fs::read(previous.join(APP)).unwrap(),
            b"previous executable"
        );
    }

    #[test]
    fn installer_only_resolves_sibling_directories() {
        let root = tempfile::tempdir().unwrap();
        let work = root.path().join(".blazer-update-fixture");
        fs::create_dir(&work).unwrap();
        let helper = work.join(HELPER);
        assert_eq!(
            helper_paths(&helper, OsStr::new("client")).unwrap().1,
            root.path().join("client")
        );
        assert!(helper_paths(&helper, OsStr::new("..\\client")).is_err());
        assert!(helper_paths(&helper, OsStr::new(".blazer-update-fixture")).is_err());
        assert!(helper_paths(&root.path().join(APP), OsStr::new("client")).is_err());
    }
}
