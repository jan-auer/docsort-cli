use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

/// Represents a single inbox directory with a display label.
#[derive(Debug, Deserialize, Clone)]
pub struct InboxConfig {
    /// Human-readable label shown in the UI.
    pub label: String,
    /// Filesystem path to the inbox directory.
    pub path: PathBuf,
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

/// Searches for `.docsort.toml` in cwd first, then the home directory.
///
/// Returns the path of the first config file found, or `None` if neither exists.
pub fn find_default_config() -> Option<PathBuf> {
    let cwd_candidate = PathBuf::from(".docsort.toml");
    if cwd_candidate.exists() {
        return Some(cwd_candidate);
    }

    let home_candidate = dirs_next_home().map(|h| h.join(".docsort.toml"))?;
    if home_candidate.exists() {
        Some(home_candidate)
    } else {
        None
    }
}

/// Returns the current user's home directory using the `HOME` environment variable.
fn dirs_next_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
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
        assert_eq!(config.inboxes[0].label, "Digital");
        assert_eq!(config.inboxes[0].path, PathBuf::from("/tmp/inbox"));
        assert_eq!(config.inboxes[1].label, "Scans");
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
}
