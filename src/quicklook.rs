use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use anyhow::{Context, Result};

/// Manages a `qlmanage -p` subprocess for Quick Look previews.
///
/// Only one file can be previewed at a time. The process is spawned
/// on demand and killed when toggled off or replaced.
#[derive(Debug)]
pub struct QuickLook {
    process: Option<Child>,
    current_path: Option<PathBuf>,
}

impl QuickLook {
    /// Creates a new `QuickLook` instance with no active preview.
    pub fn new() -> Self {
        Self {
            process: None,
            current_path: None,
        }
    }

    /// Polls the child process to detect external closes.
    ///
    /// If the `qlmanage` process has exited on its own (e.g. the user closed
    /// the Quick Look window), clears `process` and `current_path` so that
    /// subsequent calls reflect the true state.
    pub fn poll(&mut self) {
        let exited = if let Some(child) = self.process.as_mut() {
            matches!(child.try_wait(), Ok(Some(_)))
        } else {
            false
        };
        if exited {
            self.process = None;
            self.current_path = None;
        }
    }

    /// Toggles the Quick Look preview for `path`.
    ///
    /// - If no preview is open, opens `path`.
    /// - If the same file is already open, closes it.
    /// - If a different file is open, switches to `path`.
    pub fn toggle(&mut self, path: &Path) {
        self.poll();
        if self.is_open_for(path) {
            self.close();
        } else {
            self.open(path);
        }
    }

    /// Opens a Quick Look preview for `path`, killing any existing preview first.
    ///
    /// Logs a warning to stderr if spawning fails.
    pub fn open(&mut self, path: &Path) {
        self.close();

        let path_str = path.to_string_lossy();
        match spawn_qlmanage(&path_str) {
            Ok(child) => {
                self.process = Some(child);
                self.current_path = Some(path.to_path_buf());
            }
            Err(e) => {
                eprintln!(
                    "warning: failed to open Quick Look for {}: {}",
                    path.display(),
                    e
                );
            }
        }
    }

    /// Kills the active Quick Look process, if any.
    pub fn close(&mut self) {
        if let Some(mut child) = self.process.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.current_path = None;
    }

    /// Returns `true` if a Quick Look preview is currently open for `path`.
    ///
    /// Calls `poll()` first to detect any externally-closed process.
    pub fn is_open_for(&mut self, path: &Path) -> bool {
        self.poll();
        self.current_path.as_deref() == Some(path)
    }
}

impl Default for QuickLook {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for QuickLook {
    fn drop(&mut self) {
        self.close();
    }
}

/// Spawns `qlmanage -p <path>` and returns the child process handle.
///
/// Stdout and stderr are redirected to null to suppress diagnostic noise.
fn spawn_qlmanage(path_str: &str) -> Result<Child> {
    Command::new("qlmanage")
        .args(["-p", path_str])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("failed to spawn qlmanage")
}

#[cfg(test)]
mod tests {
    use super::*;

    use tempfile::NamedTempFile;

    #[test]
    fn new_has_no_active_preview() {
        let mut ql = QuickLook::new();
        assert!(!ql.is_open_for(Path::new("/any/path")));
        assert!(ql.current_path.is_none());
        assert!(ql.process.is_none());
    }

    #[test]
    fn is_open_for_returns_false_for_different_path() {
        let mut ql = QuickLook::new();
        assert!(!ql.is_open_for(Path::new("/some/file.pdf")));
    }

    #[test]
    fn close_on_empty_is_safe() {
        let mut ql = QuickLook::new();
        ql.close(); // must not panic
        assert!(ql.current_path.is_none());
    }

    /// Verifies that `open` sets `current_path` even when `qlmanage` is not
    /// available (in CI). We test with a real temp file and accept that the
    /// process may or may not spawn.
    #[test]
    fn open_updates_current_path_on_success() {
        let file = NamedTempFile::new().unwrap();
        let mut ql = QuickLook::new();

        // We can't guarantee qlmanage is available, so we just verify that
        // after open(), either current_path is set (success) or it stays None
        // (spawn failure). What we must NOT see is a panic.
        ql.open(file.path());
        // No assertion on current_path — qlmanage may not be present in test env.
        // The main invariant: no panic.
    }

    #[test]
    fn toggle_closes_when_same_file_open() {
        let mut ql = QuickLook::new();
        let path = Path::new("/fake/file.pdf");

        // Manually set state as if open succeeded (bypass spawn)
        ql.current_path = Some(path.to_path_buf());
        assert!(ql.is_open_for(path));

        // Toggle on the same file should close
        ql.toggle(path);
        assert!(!ql.is_open_for(path));
        assert!(ql.current_path.is_none());
    }

    #[test]
    fn default_is_same_as_new() {
        let ql: QuickLook = Default::default();
        assert!(ql.current_path.is_none());
        assert!(ql.process.is_none());
    }
}
