use anyhow::{Context, Result, bail};
use fs2::FileExt;
use std::{
    fs::{self, File, OpenOptions},
    io::{Error, ErrorKind},
    path::Path,
    thread,
    time::{Duration, Instant},
};

pub struct StartupLock {
    file: File,
}

impl StartupLock {
    pub fn acquire(path: &Path, timeout: Duration) -> Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .with_context(|| format!("failed to open startup lock: {}", path.display()))?;
        let started = Instant::now();
        loop {
            match file.try_lock_exclusive() {
                Ok(()) => return Ok(Self { file }),
                Err(error) if is_lock_contention(&error) => {
                    if started.elapsed() >= timeout {
                        bail!(
                            "timed out waiting {} seconds for owned emulator startup lock: {}",
                            timeout.as_secs(),
                            path.display()
                        );
                    }
                    thread::sleep(Duration::from_millis(200));
                }
                Err(error) => return Err(error).context("failed to acquire startup lock"),
            }
        }
    }
}

fn is_lock_contention(error: &Error) -> bool {
    if error.kind() == ErrorKind::WouldBlock {
        return true;
    }

    #[cfg(windows)]
    {
        // fs2 maps LockFileEx contention to the raw Win32 codes instead of
        // ErrorKind::WouldBlock: 32 = sharing violation, 33 = lock violation.
        matches!(error.raw_os_error(), Some(32 | 33))
    }

    #[cfg(not(windows))]
    false
}

impl Drop for StartupLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_startup_lock_serializes_and_releases() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("target")
            .join(format!("startup-lock-test-{}.lock", std::process::id()));
        let first = StartupLock::acquire(&path, Duration::from_secs(1)).unwrap();

        let started = Instant::now();
        let blocked = StartupLock::acquire(&path, Duration::from_millis(80));
        let elapsed = started.elapsed();

        let error = match blocked {
            Ok(_) => panic!("second startup lock unexpectedly succeeded"),
            Err(error) => error.to_string(),
        };
        assert!(
            elapsed >= Duration::from_millis(60),
            "lock contention returned immediately after {elapsed:?}: {error}"
        );
        assert!(error.contains("timed out waiting"), "{error}");
        drop(first);
        let second = StartupLock::acquire(&path, Duration::from_secs(1));
        assert!(second.is_ok());
        drop(second);
        let _ = fs::remove_file(path);
    }
}
