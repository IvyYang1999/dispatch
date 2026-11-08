//! Refresh an existing CLI without re-registering hooks or MCP servers.
use anyhow::{bail, Context, Result};
use semver::Version;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Default)]
pub struct State(Mutex<Option<Result<(), String>>>);

impl State {
    /// Once per launch, on the blocking pool. Settings waits for the same result.
    pub fn ensure(&self, source: Option<&Path>, target: &Path) -> Option<String> {
        let mut result = self.0.lock().unwrap_or_else(|e| e.into_inner());
        result
            .get_or_insert_with(|| {
                if cfg!(debug_assertions) {
                    return Ok(()); // `tauri dev` must not replace a production CLI.
                }
                refresh(source.unwrap_or_else(|| Path::new("")), target, binary_version).map_err(|e| format!("{e:#}"))
            })
            .as_ref()
            .err()
            .cloned()
    }

    pub fn installed(&self) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = Some(Ok(()));
    }
}

fn binary_version(binary: &Path) -> Result<Version> {
    // A file avoids waiting on an inherited stdout pipe. The process has a deadline.
    let mut output = tempfile::tempfile()?;
    let mut command = Command::new(binary);
    command.arg("--version").stdin(Stdio::null()).stderr(Stdio::null()).stdout(output.try_clone()?);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000); // No console window on background launch.
    }
    let mut child = command.spawn().context("read CLI version")?;
    let until = Instant::now() + Duration::from_secs(3);
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break,
            Ok(Some(_)) => bail!("CLI --version failed"),
            Ok(None) if Instant::now() < until => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                bail!("CLI --version did not finish");
            }
        }
    }
    output.seek(SeekFrom::Start(0))?;
    let mut text = String::new();
    output.take(128).read_to_string(&mut text)?;
    Version::parse(text.trim().strip_prefix("dispatch ").context("unexpected CLI version response")?).context("invalid CLI version")
}

fn refresh(source: &Path, target: &Path, version: impl Fn(&Path) -> Result<Version>) -> Result<()> {
    let meta = match fs::symlink_metadata(target) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
    };
    if !meta.file_type().is_file() {
        bail!("installed CLI is not a regular file; use Reinstall in Settings");
    }
    let old = fs::read(target)?;
    let new = fs::read(source).context("read bundled CLI")?;
    if old == new || version(source)? < version(target)? {
        return Ok(());
    }
    let staged = tempfile::Builder::new().prefix(".dispatch-").tempfile_in(target.parent().context("CLI directory")?)?;
    fs::copy(source, staged.path())?;
    staged.as_file().sync_all()?;
    if fs::read(staged.path())? != new || fs::read(target)? != old {
        bail!("CLI changed during refresh; try Reinstall in Settings");
    }
    // Never truncate the executable that a running MCP server may still be using.
    staged.persist(target).map_err(|e| e.error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn versions(p: &Path) -> Result<Version> {
        Ok(Version::parse(fs::read_to_string(p)?.trim())?)
    }

    #[test]
    fn refreshes_an_older_cli_and_preserves_other_files() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("bundled");
        let target = dir.path().join("dispatch");
        let config = dir.path().join("settings.json");
        fs::write(&source, "0.1.5").unwrap();
        fs::write(&target, "0.1.4").unwrap();
        fs::write(&config, "keep hooks and MCP").unwrap();
        refresh(&source, &target, versions).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"0.1.5");
        assert_eq!(fs::read(&config).unwrap(), b"keep hooks and MCP");
        refresh(&source, &target, |_| panic!("identical CLI must be a no-op")).unwrap();
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 3);
    }

    #[test]
    fn does_not_install_on_first_launch_or_downgrade_a_newer_cli() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("bundled");
        let target = dir.path().join("dispatch");
        fs::write(&source, "0.1.5").unwrap();
        refresh(&source, &target, |_| panic!("not installed")).unwrap();
        assert!(!target.exists());
        fs::write(&target, "0.2.0").unwrap();
        refresh(&source, &target, versions).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"0.2.0");
    }

    #[test]
    fn missing_or_invalid_source_preserves_installed_cli() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("missing");
        let target = dir.path().join("dispatch");
        fs::write(&target, "0.1.4").unwrap();
        assert!(refresh(&source, &target, versions).is_err());
        fs::write(&source, "invalid binary").unwrap();
        assert!(refresh(&source, &target, versions).is_err());
        assert_eq!(fs::read(&target).unwrap(), b"0.1.4");
    }

    #[test]
    fn same_version_rebuild_is_refreshed_by_content() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("bundled");
        let target = dir.path().join("dispatch");
        fs::write(&source, "new build").unwrap();
        fs::write(&target, "old build").unwrap();
        refresh(&source, &target, |_| Ok(Version::new(0, 1, 5))).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"new build");
    }

    #[cfg(unix)]
    #[test]
    fn version_probe_handles_real_process_success_failure_and_timeout() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let binary = dir.path().join("dispatch");
        fs::write(&binary, "#!/bin/sh\nprintf 'dispatch 0.1.5\\n'\n").unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(binary_version(&binary).unwrap(), Version::new(0, 1, 5));
        fs::write(&binary, "#!/bin/sh\nexit 7\n").unwrap();
        assert!(binary_version(&binary).is_err());
        fs::write(&binary, "#!/bin/sh\nexec sleep 30\n").unwrap();
        let start = Instant::now();
        assert!(binary_version(&binary).is_err());
        assert!(start.elapsed() < Duration::from_secs(5));
    }

    #[cfg(unix)]
    #[test]
    fn replacement_is_atomic_and_executable_and_rejects_symlinks() {
        use std::io::Read;
        use std::os::unix::fs::{symlink, PermissionsExt};
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("bundled");
        let target = dir.path().join("dispatch");
        fs::write(&source, "0.1.5").unwrap();
        fs::set_permissions(&source, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(&target, "0.1.4").unwrap();
        let mut running = fs::File::open(&target).unwrap();
        refresh(&source, &target, versions).unwrap();
        let mut old = String::new();
        running.read_to_string(&mut old).unwrap();
        assert_eq!(old, "0.1.4");
        assert_eq!(fs::read(&target).unwrap(), b"0.1.5");
        assert_eq!(fs::metadata(&target).unwrap().permissions().mode() & 0o777, 0o755);
        fs::remove_file(&target).unwrap();
        symlink(&source, &target).unwrap();
        assert!(refresh(&source, &target, versions).is_err());
        assert!(target.is_symlink());
    }
}
