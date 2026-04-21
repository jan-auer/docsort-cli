use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

/// Represents a single inbox directory with an optional display label.
#[derive(Debug, Deserialize, Clone)]
pub struct InboxConfig {
    /// Human-readable label shown in the UI. Defaults to the folder name when absent.
    pub label: Option<String>,
    /// Filesystem path to the inbox directory.
    pub path: PathBuf,
}

/// Returns the display label for an inbox.
///
/// Uses the configured label if present, otherwise falls back to the last
/// path component (the folder name). If the path has no file name component,
/// falls back to the full path as a string.
pub fn inbox_label(inbox: &InboxConfig) -> &str {
    if let Some(label) = &inbox.label {
        return label.as_str();
    }
    inbox
        .path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_else(|| inbox.path.to_str().unwrap_or(""))
}

/// Holds destination archive configuration.
#[derive(Debug, Deserialize, Clone)]
pub struct DestinationConfig {
    /// Root directory of the destination archive tree.
    pub root: PathBuf,
}

/// Top-level application configuration loaded from `.docsort.toml`.
#[derive(Debug, Deserialize, Clone)]
pub struct Config {
    /// List of inbox directories to scan for unsorted files.
    pub inboxes: Vec<InboxConfig>,
    /// Destination archive configuration.
    pub destination: DestinationConfig,
}

/// Loads and deserializes a `Config` from the TOML file at `path`.
///
/// Returns an error if the file cannot be read or parsed.
pub fn load_config(path: &Path) -> Result<Config> {
    let contents = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read config file: {}", path.display()))?;
    let config: Config = toml::from_str(&contents)
        .with_context(|| format!("failed to parse config file: {}", path.display()))?;
    Ok(config)
}

/// Searches for `.docsort.toml` by walking up from cwd to root.
///
/// Walks up the directory tree from the current working directory to the filesystem root,
/// checking each directory for a `.docsort.toml` file. Silently skips any directory where
/// reading is not permitted (permission errors). Returns the path of the first config file
/// found, or `None` if none exists.
pub fn find_default_config() -> Option<PathBuf> {
    let mut current = std::env::current_dir().ok()?;

    loop {
        let candidate = current.join(".docsort.toml");
        if candidate.exists() {
            return Some(candidate);
        }

        // Try to move to parent directory. If we reach the root without finding a parent,
        // stop searching.
        let parent = current.parent();
        match parent {
            Some(p) if p != current => {
                current = p.to_path_buf();
            }
            _ => {
                // We've reached the root, or there's no parent (shouldn't happen on Unix).
                break;
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::io::Write;

    use tempfile::NamedTempFile;

    #[test]
    fn load_config_parses_valid_toml() {
        let mut file = NamedTempFile::new().unwrap();
        write!(
            file,
            r#"
[[inboxes]]
label = "Digital"
path = "/tmp/inbox"

[[inboxes]]
label = "Scans"
path = "/tmp/scans"

[destination]
root = "/tmp/archive"
"#
        )
        .unwrap();

        let config = load_config(file.path()).expect("should parse valid config");
        assert_eq!(config.inboxes.len(), 2);
        assert_eq!(config.inboxes[0].label, Some("Digital".to_string()));
        assert_eq!(config.inboxes[0].path, PathBuf::from("/tmp/inbox"));
        assert_eq!(config.inboxes[1].label, Some("Scans".to_string()));
        assert_eq!(config.destination.root, PathBuf::from("/tmp/archive"));
    }

    #[test]
    fn load_config_errors_on_missing_file() {
        let result = load_config(Path::new("/nonexistent/path/.docsort.toml"));
        assert!(result.is_err());
        let msg = format!("{}", result.unwrap_err());
        assert!(msg.contains("failed to read config file"));
    }

    #[test]
    fn load_config_errors_on_invalid_toml() {
        let mut file = NamedTempFile::new().unwrap();
        write!(file, "not valid toml ][[[").unwrap();
        let result = load_config(file.path());
        assert!(result.is_err());
        let msg = format!("{}", result.unwrap_err());
        assert!(msg.contains("failed to parse config file"));
    }

    #[test]
    fn inbox_label_returns_configured_label_when_set() {
        let inbox = InboxConfig {
            label: Some("My Scans".to_string()),
            path: PathBuf::from("/tmp/scans"),
        };
        assert_eq!(inbox_label(&inbox), "My Scans");
    }

    #[test]
    fn inbox_label_falls_back_to_folder_name_when_absent() {
        let inbox = InboxConfig {
            label: None,
            path: PathBuf::from("/tmp/scans"),
        };
        assert_eq!(inbox_label(&inbox), "scans");
    }
}
