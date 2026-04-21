use std::cmp::Reverse;
use std::path::PathBuf;
use std::time::SystemTime;

use anyhow::{Context, Result};

use crate::config::{inbox_label, Config};

/// Represents a single file found in an inbox directory.
#[derive(Debug, Clone)]
pub struct InboxFile {
    /// The label of the inbox this file belongs to, from config.
    pub label: String,
    /// Full filesystem path to the file.
    pub path: PathBuf,
    /// File name only (no directory component).
    pub filename: String,
    /// Last-modified time, used for sorting.
    pub modified: SystemTime,
}

/// Scans all configured inbox directories and returns their top-level files.
///
/// Results are sorted newest-first by modification time. Directories and
/// symlinks within inbox directories are skipped. Missing inbox directories
/// are skipped with a warning rather than treated as errors.
pub fn scan_inboxes(config: &Config) -> Result<Vec<InboxFile>> {
    let mut files: Vec<InboxFile> = Vec::new();

    for inbox in &config.inboxes {
        let read_dir = match std::fs::read_dir(&inbox.path) {
            Ok(rd) => rd,
            Err(e) => {
                eprintln!(
                    "warning: skipping inbox '{}' ({}): {}",
                    inbox_label(inbox),
                    inbox.path.display(),
                    e
                );
                continue;
            }
        };

        let label = inbox_label(inbox).to_owned();

        for entry_result in read_dir {
            let entry = entry_result.with_context(|| {
                format!(
                    "failed to read entry in inbox '{}' ({})",
                    label,
                    inbox.path.display()
                )
            })?;

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

            // Skip hidden files and directories
            if filename.starts_with('.') {
                continue;
            }

            files.push(InboxFile {
                label: label.clone(),
                path: entry.path(),
                filename,
                modified,
            });
        }
    }

    files.sort_by_key(|b| Reverse(b.modified));

    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;
    use std::io::Write;

    use tempfile::TempDir;

    use crate::config::{Config, DestinationConfig, InboxConfig};

    fn make_config(inbox_paths: Vec<(&str, &str)>, dest: &str) -> Config {
        Config {
            inboxes: inbox_paths
                .into_iter()
                .map(|(label, path)| InboxConfig {
                    label: Some(label.to_string()),
                    path: PathBuf::from(path),
                })
                .collect(),
            destination: DestinationConfig {
                root: PathBuf::from(dest),
            },
        }
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

        let config = make_config(vec![("Inbox", dir.path().to_str().unwrap())], "/tmp/dest");
        let files = scan_inboxes(&config).expect("scan should succeed");

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

        let config = make_config(vec![("Inbox", dir.path().to_str().unwrap())], "/tmp/dest");
        let files = scan_inboxes(&config).expect("scan should succeed");

        assert_eq!(files.len(), 1);
        assert_eq!(files[0].filename, "file.txt");
    }

    #[test]
    fn scan_inboxes_skips_missing_inbox_with_warning() {
        let config = make_config(vec![("Missing", "/nonexistent/inbox/path")], "/tmp/dest");
        let files = scan_inboxes(&config).expect("should not error on missing inbox");
        assert!(files.is_empty());
    }

    #[test]
    fn scan_inboxes_combines_multiple_inboxes() {
        let dir_a = TempDir::new().unwrap();
        let dir_b = TempDir::new().unwrap();

        fs::write(dir_a.path().join("one.pdf"), b"pdf").unwrap();
        fs::write(dir_b.path().join("two.pdf"), b"pdf").unwrap();

        let config = make_config(
            vec![
                ("A", dir_a.path().to_str().unwrap()),
                ("B", dir_b.path().to_str().unwrap()),
            ],
            "/tmp/dest",
        );
        let files = scan_inboxes(&config).expect("scan should succeed");
        assert_eq!(files.len(), 2);
    }

    #[test]
    fn inbox_file_carries_correct_label() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("doc.txt"), b"x").unwrap();

        let config = make_config(vec![("MyLabel", dir.path().to_str().unwrap())], "/tmp/dest");
        let files = scan_inboxes(&config).unwrap();
        assert_eq!(files[0].label, "MyLabel");
    }

    #[test]
    fn scan_inboxes_skips_hidden_files() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("visible.txt"), b"data").unwrap();
        fs::write(dir.path().join(".hidden"), b"hidden").unwrap();
        fs::write(dir.path().join(".DS_Store"), b"system").unwrap();

        let config = make_config(vec![("Inbox", dir.path().to_str().unwrap())], "/tmp/dest");
        let files = scan_inboxes(&config).expect("scan should succeed");

        assert_eq!(files.len(), 1);
        assert_eq!(files[0].filename, "visible.txt");
    }
}
