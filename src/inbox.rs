use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context, Result};
use walkdir::WalkDir;

use crate::config::{inbox_label, InboxConfig};

/// Represents a single file found in an inbox directory.
#[derive(Debug, Clone)]
pub struct InboxFile {
    /// The label of the inbox this file belongs to, from config.
    pub label: String,
    /// Full filesystem path to the file.
    pub path: PathBuf,
    /// File name only (no directory component).
    pub filename: String,
    /// Last-modified time of the file itself, used for within-group sorting.
    pub modified: SystemTime,
    /// Relative path from the inbox root to the file's parent directory, e.g.
    /// `"ProjectA/Client Work/"`. Always ends with `/` when non-empty.
    /// `None` for files directly inside the inbox root.
    pub subfolder: Option<String>,
    /// Sort key for the group this file belongs to.
    ///
    /// For top-level files this equals `modified`. For subfolder files this is
    /// the parent folder's own modification time (falling back to creation time
    /// when `modified()` is unavailable). All files in the same subfolder share
    /// the same `group_sort_key`, so they stay together when the list is sorted.
    pub group_sort_key: SystemTime,
}

/// Scans all configured inbox directories and returns their files recursively.
///
/// Files are sorted as follows:
/// - Top-level files and subfolders are interleaved in newest-first order by
///   their respective sort keys: a top-level file's sort key is its own
///   modification time; a subfolder's sort key is the folder's modification
///   time (falling back to creation time).
/// - Within each subfolder group, files are sorted newest-first by their own
///   modification time.
/// - Moved files at the end (handled by the caller via `refresh_files`).
///
/// Hidden directories (names starting with `.`) are skipped, as are hidden
/// files. Missing inbox directories are skipped with a warning.
pub fn scan_inboxes(inboxes: &[InboxConfig]) -> Result<Vec<InboxFile>> {
    let mut files: Vec<InboxFile> = Vec::new();

    for inbox in inboxes {
        let label = inbox_label(inbox).to_owned();
        let inbox_root = &inbox.path;

        let mut walker = WalkDir::new(inbox_root)
            .min_depth(1)
            .into_iter()
            .filter_entry(|e| !e.file_name().to_string_lossy().starts_with('.'));

        // Peek at the first result to detect misconfigured inbox paths before
        // proceeding. WalkDir surfaces the root-level open error as the first
        // item in the iterator, so we can inspect it here and emit a targeted
        // diagnostic rather than a generic per-entry warning.
        let first = walker.next();
        let first = match first {
            None => {
                // Empty inbox — nothing to do.
                continue;
            }
            Some(Err(ref walk_err)) => {
                let io_kind = walk_err
                    .io_error()
                    .map(io::Error::kind)
                    .unwrap_or(io::ErrorKind::Other);
                match io_kind {
                    io::ErrorKind::NotFound => {
                        eprintln!(
                            "warning: skipping inbox '{}' ({}): directory not found",
                            label,
                            inbox_root.display(),
                        );
                        continue;
                    }
                    io::ErrorKind::NotADirectory => {
                        eprintln!(
                            "warning: skipping inbox '{}' ({}): path is not a directory",
                            label,
                            inbox_root.display(),
                        );
                        continue;
                    }
                    _ => {
                        eprintln!("warning: skipping entry in inbox '{}': {}", label, walk_err);
                        continue;
                    }
                }
            }
            Some(ok) => ok,
        };

        for entry_result in std::iter::once(first).chain(walker) {
            let entry = match entry_result {
                Ok(e) => e,
                Err(e) => {
                    eprintln!("warning: skipping entry in inbox '{}': {}", label, e);
                    continue;
                }
            };

            let metadata = entry.metadata().with_context(|| {
                format!("failed to read metadata for {}", entry.path().display())
            })?;

            if !metadata.is_file() {
                continue;
            }

            let modified = metadata.modified().with_context(|| {
                format!(
                    "failed to read modification time for {}",
                    entry.path().display()
                )
            })?;

            let filename = entry.file_name().to_string_lossy().into_owned();

            // Compute the subfolder relative to the inbox root.
            let subfolder = compute_subfolder(inbox_root, entry.path());

            // Sort key: for top-level files use the file's own modification
            // time; for subfolder files use the immediate top-level folder's
            // modification time (falling back to created()).
            let group_sort_key = match &subfolder {
                None => modified,
                Some(_) => {
                    let top_level_dir = top_level_dir(inbox_root, entry.path());
                    folder_sort_key(&top_level_dir).unwrap_or_else(|e| {
                        eprintln!(
                            "warning: failed to read sort key for '{}': {e}",
                            top_level_dir.display()
                        );
                        SystemTime::UNIX_EPOCH
                    })
                }
            };

            files.push(InboxFile {
                label: label.clone(),
                path: entry.path().to_path_buf(),
                filename,
                modified,
                subfolder,
                group_sort_key,
            });
        }
    }

    sort_inbox_files(&mut files);

    Ok(files)
}

/// Computes the subfolder path relative to `inbox_root` for a given `file_path`.
///
/// Returns `None` for files directly inside `inbox_root`. For files in
/// subdirectories, returns the relative path with a trailing `/`,
/// e.g. `"ProjectA/Client Work/"`.
fn compute_subfolder(inbox_root: &Path, file_path: &Path) -> Option<String> {
    let parent = file_path.parent()?;
    if parent == inbox_root {
        return None;
    }
    let rel = parent.strip_prefix(inbox_root).ok()?;
    let rel_str = rel.to_string_lossy();
    if rel_str.is_empty() {
        None
    } else {
        Some(format!("{rel_str}/"))
    }
}

/// Returns the path of the immediate child of `inbox_root` that contains
/// `file_path`. For top-level files this is the file itself; for deeply nested
/// files it is the first path component below `inbox_root`.
fn top_level_dir(inbox_root: &Path, file_path: &Path) -> PathBuf {
    let rel = match file_path.strip_prefix(inbox_root) {
        Ok(r) => r,
        Err(_) => return file_path.to_path_buf(),
    };
    let first_component = rel.components().next();
    match first_component {
        Some(c) => inbox_root.join(c),
        None => file_path.to_path_buf(),
    }
}

/// Returns the sort key for a folder: `modified()`, falling back to `created()`.
///
/// Returns an error if neither time is available.
fn folder_sort_key(folder_path: &Path) -> Result<SystemTime> {
    let meta = std::fs::metadata(folder_path).with_context(|| {
        format!(
            "failed to read metadata for folder {}",
            folder_path.display()
        )
    })?;
    meta.modified()
        .or_else(|_| meta.created())
        .with_context(|| {
            format!(
                "failed to read modification or creation time for folder {}",
                folder_path.display()
            )
        })
}

/// Sorts inbox files so that top-level files and subfolders are interleaved
/// newest-first by `group_sort_key`. Within each group sharing the same key,
/// files are sorted newest-first by their own `modified` time.
pub fn sort_inbox_files(files: &mut [InboxFile]) {
    files.sort_by(|a, b| {
        // Primary: newest group first (descending).
        let key_ord = b.group_sort_key.cmp(&a.group_sort_key);
        if key_ord != std::cmp::Ordering::Equal {
            return key_ord;
        }
        // Secondary: within the same group, newest file first.
        b.modified.cmp(&a.modified)
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;
    use std::io::Write;

    use tempfile::TempDir;

    use crate::config::InboxConfig;

    fn make_inboxes(inbox_paths: Vec<(&str, &str)>) -> Vec<InboxConfig> {
        inbox_paths
            .into_iter()
            .map(|(label, path)| InboxConfig {
                label: Some(label.to_string()),
                path: PathBuf::from(path),
            })
            .collect()
    }

    #[test]
    fn scan_inboxes_returns_files_sorted_newest_first() {
        let dir = TempDir::new().unwrap();
        let file_a = dir.path().join("a.txt");
        let file_b = dir.path().join("b.txt");

        fs::write(&file_a, b"aaa").unwrap();
        // Ensure b is modified later than a
        std::thread::sleep(std::time::Duration::from_millis(10));
        fs::write(&file_b, b"bbb").unwrap();

        let inboxes = make_inboxes(vec![("Inbox", dir.path().to_str().unwrap())]);
        let files = scan_inboxes(&inboxes).expect("scan should succeed");

        assert_eq!(files.len(), 2);
        // newest first
        assert_eq!(files[0].filename, "b.txt");
        assert_eq!(files[1].filename, "a.txt");
    }

    #[test]
    fn scan_inboxes_skips_directories() {
        let dir = TempDir::new().unwrap();
        let subdir = dir.path().join("subdir");
        fs::create_dir(&subdir).unwrap();
        let mut file = fs::File::create(dir.path().join("file.txt")).unwrap();
        file.write_all(b"data").unwrap();

        let inboxes = make_inboxes(vec![("Inbox", dir.path().to_str().unwrap())]);
        let files = scan_inboxes(&inboxes).expect("scan should succeed");

        assert_eq!(files.len(), 1);
        assert_eq!(files[0].filename, "file.txt");
    }

    #[test]
    fn scan_inboxes_skips_missing_inbox_with_warning() {
        let inboxes = make_inboxes(vec![("Missing", "/nonexistent/inbox/path")]);
        let files = scan_inboxes(&inboxes).expect("should not error on missing inbox");
        assert!(files.is_empty());
    }

    /// When the configured inbox path points to a file rather than a directory,
    /// `scan_inboxes` should skip it with a clear warning and continue — not
    /// silently emit per-entry errors or panic.
    #[test]
    fn scan_inboxes_skips_inbox_that_is_a_file() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("not_a_dir.txt");
        fs::write(&file_path, b"data").unwrap();

        let inboxes = make_inboxes(vec![("NotADir", file_path.to_str().unwrap())]);
        let files = scan_inboxes(&inboxes).expect("should not error when inbox is a file");
        assert!(files.is_empty());
    }

    #[test]
    fn scan_inboxes_combines_multiple_inboxes() {
        let dir_a = TempDir::new().unwrap();
        let dir_b = TempDir::new().unwrap();

        fs::write(dir_a.path().join("one.pdf"), b"pdf").unwrap();
        fs::write(dir_b.path().join("two.pdf"), b"pdf").unwrap();

        let inboxes = make_inboxes(vec![
            ("A", dir_a.path().to_str().unwrap()),
            ("B", dir_b.path().to_str().unwrap()),
        ]);
        let files = scan_inboxes(&inboxes).expect("scan should succeed");
        assert_eq!(files.len(), 2);
    }

    #[test]
    fn inbox_file_carries_correct_label() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("doc.txt"), b"x").unwrap();

        let inboxes = make_inboxes(vec![("MyLabel", dir.path().to_str().unwrap())]);
        let files = scan_inboxes(&inboxes).unwrap();
        assert_eq!(files[0].label, "MyLabel");
    }

    #[test]
    fn scan_inboxes_skips_hidden_files() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("visible.txt"), b"data").unwrap();
        fs::write(dir.path().join(".hidden"), b"hidden").unwrap();
        fs::write(dir.path().join(".DS_Store"), b"system").unwrap();

        let inboxes = make_inboxes(vec![("Inbox", dir.path().to_str().unwrap())]);
        let files = scan_inboxes(&inboxes).expect("scan should succeed");

        assert_eq!(files.len(), 1);
        assert_eq!(files[0].filename, "visible.txt");
    }

    /// A top-level file modified *after* the subfolder was created must appear
    /// before the subfolder's files in the sorted list.
    #[test]
    fn subfolder_files_interleaved_with_top_level_by_folder_date() {
        let dir = TempDir::new().unwrap();

        // Create the subfolder first (older date).
        let subfolder = dir.path().join("OldFolder");
        fs::create_dir(&subfolder).unwrap();
        fs::write(subfolder.join("notes.txt"), b"note").unwrap();

        // Write the top-level file after the subfolder exists (newer date).
        std::thread::sleep(std::time::Duration::from_millis(20));
        fs::write(dir.path().join("top.txt"), b"top").unwrap();

        let inboxes = make_inboxes(vec![("Inbox", dir.path().to_str().unwrap())]);
        let files = scan_inboxes(&inboxes).expect("scan should succeed");

        assert_eq!(files.len(), 2);
        // The top-level file is newer, so it must come first.
        assert_eq!(files[0].filename, "top.txt");
        assert_eq!(files[1].filename, "notes.txt");
    }

    /// A file nested three levels deep (`A/B/C/file.txt`) must have
    /// `subfolder` set to `"A/B/C/"` and its `group_sort_key` equal to the
    /// modification time of the top-level directory `A/` — not of `B/`, `C/`,
    /// or the file itself.
    #[test]
    fn deep_nesting_subfolder_and_group_sort_key() {
        let dir = TempDir::new().unwrap();

        let a = dir.path().join("A");
        let ab = a.join("B");
        let abc = ab.join("C");
        fs::create_dir_all(&abc).unwrap();
        fs::write(abc.join("file.txt"), b"data").unwrap();

        let inboxes = make_inboxes(vec![("Inbox", dir.path().to_str().unwrap())]);
        let files = scan_inboxes(&inboxes).expect("scan should succeed");

        assert_eq!(files.len(), 1);
        let f = &files[0];
        assert_eq!(f.filename, "file.txt");
        assert_eq!(f.subfolder.as_deref(), Some("A/B/C/"));

        let top_level_key = folder_sort_key(&a).unwrap();
        assert_eq!(f.group_sort_key, top_level_key);
    }

    /// Two files in the same subfolder must be adjacent in the sorted result —
    /// they must not be separated by a top-level file that falls between them
    /// in modification time.
    #[test]
    fn subfolder_files_stay_grouped_after_sort() {
        let dir = TempDir::new().unwrap();

        let subfolder = dir.path().join("Group");
        fs::create_dir(&subfolder).unwrap();

        // Write first subfolder file, then a top-level file, then the second
        // subfolder file. Without grouping the top-level file would end up
        // between the two subfolder files.
        fs::write(subfolder.join("first.txt"), b"a").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        fs::write(dir.path().join("top.txt"), b"top").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        fs::write(subfolder.join("second.txt"), b"b").unwrap();

        let inboxes = make_inboxes(vec![("Inbox", dir.path().to_str().unwrap())]);
        let files = scan_inboxes(&inboxes).expect("scan should succeed");

        assert_eq!(files.len(), 3);

        let pos_first = files
            .iter()
            .position(|f| f.filename == "first.txt")
            .unwrap();
        let pos_second = files
            .iter()
            .position(|f| f.filename == "second.txt")
            .unwrap();
        let pos_top = files.iter().position(|f| f.filename == "top.txt").unwrap();

        // Both subfolder files must be adjacent (no top-level file between them).
        let (lo, hi) = if pos_first < pos_second {
            (pos_first, pos_second)
        } else {
            (pos_second, pos_first)
        };
        assert!(
            pos_top < lo || pos_top > hi,
            "top.txt (pos {pos_top}) must not appear between the two subfolder files (pos {lo}..{hi})"
        );
    }

    /// `sort_inbox_files` orders entries by `group_sort_key` (newest first) and
    /// within a group by `modified` (newest first), without relying on the
    /// filesystem or wall-clock time.
    #[test]
    fn sort_inbox_files_synthetic_ordering() {
        let epoch = SystemTime::UNIX_EPOCH;
        let t1 = epoch + std::time::Duration::from_secs(1);
        let t2 = epoch + std::time::Duration::from_secs(2);
        let t3 = epoch + std::time::Duration::from_secs(3);
        let t4 = epoch + std::time::Duration::from_secs(4);

        let make = |filename: &str, modified: SystemTime, group_sort_key: SystemTime| InboxFile {
            label: "Test".to_string(),
            path: PathBuf::from(filename),
            filename: filename.to_string(),
            modified,
            subfolder: None,
            group_sort_key,
        };

        // group_sort_key t3: two files in the same subfolder group
        let sub_old = make("sub_old.txt", t1, t3);
        let sub_new = make("sub_new.txt", t2, t3);
        // group_sort_key t4: a top-level file that is newer than the subfolder
        let top = make("top.txt", t4, t4);
        // group_sort_key t1: an older top-level file
        let old_top = make("old_top.txt", t1, t1);

        let mut files = vec![sub_old, sub_new, top, old_top];
        sort_inbox_files(&mut files);

        let names: Vec<&str> = files.iter().map(|f| f.filename.as_str()).collect();

        // top (t4) first, then sub_new / sub_old (group t3, newest-within-group
        // first), then old_top (t1) last.
        assert_eq!(names[0], "top.txt");
        assert_eq!(names[1], "sub_new.txt");
        assert_eq!(names[2], "sub_old.txt");
        assert_eq!(names[3], "old_top.txt");
    }

    /// When a subfolder's metadata cannot be read (e.g. no execute permission),
    /// `scan_inboxes` should warn and continue rather than aborting. Files inside
    /// the unreadable subfolder may or may not appear (walkdir may skip them), but
    /// files outside it must still be returned.
    #[cfg(unix)]
    #[test]
    fn scan_inboxes_continues_when_subfolder_metadata_unreadable() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new().unwrap();

        // Create a subfolder with a file inside it, then remove read+execute
        // permissions so that `fs::metadata` on the folder itself fails.
        let locked = dir.path().join("locked_folder");
        fs::create_dir(&locked).unwrap();
        fs::write(locked.join("inside.txt"), b"inside").unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();

        // This file lives at the inbox root and must always be returned.
        fs::write(dir.path().join("outside.txt"), b"outside").unwrap();

        let inboxes = make_inboxes(vec![("Inbox", dir.path().to_str().unwrap())]);
        let result = scan_inboxes(&inboxes);

        // Restore permissions so TempDir can clean up.
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();

        let files = result.expect("scan_inboxes must not abort when a subfolder is unreadable");
        let filenames: Vec<&str> = files.iter().map(|f| f.filename.as_str()).collect();
        assert!(
            filenames.contains(&"outside.txt"),
            "outside.txt must be present; got {filenames:?}"
        );
    }
}
