use std::cmp::Reverse;
use std::path::Path;

use anyhow::{Context, Result};
use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher};
use walkdir::WalkDir;

/// Fuzzy-searchable index of all subdirectories under the destination root.
#[derive(Debug)]
pub struct DestIndex {
    /// All relative subdirectory paths collected from the destination tree.
    dirs: Vec<String>,
}

impl DestIndex {
    /// Walks `root` and builds a fuzzy index of all subdirectories.
    ///
    /// The root itself is excluded; only its descendants are indexed.
    pub fn new(root: &Path) -> Result<Self> {
        let dirs = collect_subdirs(root)?;
        Ok(Self { dirs })
    }

    /// Returns up to 50 directory paths ranked by fuzzy match score against `pattern`.
    ///
    /// Results are sorted by descending score (best match first).
    pub fn query(&self, pattern: &str) -> Vec<String> {
        if pattern.is_empty() {
            let mut results: Vec<String> = self.dirs.clone();
            results.truncate(50);
            return results;
        }

        let mut matcher = Matcher::new(Config::DEFAULT.match_paths());
        let pat = Pattern::parse(pattern, CaseMatching::Ignore, Normalization::Smart);
        let mut scored: Vec<(String, u32)> = pat
            .match_list(self.dirs.iter().map(|s| s.as_str()), &mut matcher)
            .into_iter()
            .map(|(s, score)| (s.to_string(), score))
            .collect();

        scored.sort_by_key(|b| Reverse(b.1));
        scored.truncate(50);
        scored.into_iter().map(|(s, _)| s).collect()
    }

    /// Re-walks `root` and rebuilds the index in place.
    ///
    /// Call this after creating a new subdirectory in the destination tree.
    pub fn rebuild(&mut self, root: &Path) -> Result<()> {
        self.dirs = collect_subdirs(root)?;
        Ok(())
    }

    /// Returns the filenames inside `root/rel_path`, sorted alphabetically.
    ///
    /// Only regular files are included; directories are skipped.
    pub fn list_files(root: &Path, rel_path: &str) -> Result<Vec<String>> {
        let target = root.join(rel_path);
        let read_dir = std::fs::read_dir(&target)
            .with_context(|| format!("failed to read directory: {}", target.display()))?;

        let mut names: Vec<String> = Vec::new();
        for entry_result in read_dir {
            let entry = entry_result
                .with_context(|| format!("failed to read entry in {}", target.display()))?;
            let metadata = entry.metadata().with_context(|| {
                format!("failed to read metadata for {}", entry.path().display())
            })?;
            if metadata.is_file() {
                names.push(entry.file_name().to_string_lossy().into_owned());
            }
        }

        names.sort();
        Ok(names)
    }
}

/// Walks `root` and collects all descendant subdirectories as relative path strings.
fn collect_subdirs(root: &Path) -> Result<Vec<String>> {
    let mut dirs: Vec<String> = Vec::new();

    for entry_result in WalkDir::new(root).min_depth(1).into_iter() {
        let entry = entry_result
            .with_context(|| format!("failed to walk directory: {}", root.display()))?;

        if !entry.file_type().is_dir() {
            continue;
        }

        let rel = entry
            .path()
            .strip_prefix(root)
            .with_context(|| {
                format!(
                    "entry {} is not under root {}",
                    entry.path().display(),
                    root.display()
                )
            })?
            .to_string_lossy()
            .into_owned();

        dirs.push(rel);
    }

    Ok(dirs)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;

    use tempfile::TempDir;

    fn setup_tree(root: &Path) {
        // root/
        //   alpha/
        //     beta/
        //   gamma/
        fs::create_dir(root.join("alpha")).unwrap();
        fs::create_dir(root.join("alpha/beta")).unwrap();
        fs::create_dir(root.join("gamma")).unwrap();
    }

    #[test]
    fn new_collects_subdirs_excluding_root() {
        let dir = TempDir::new().unwrap();
        setup_tree(dir.path());

        let index = DestIndex::new(dir.path()).expect("index should build");
        assert!(index.dirs.contains(&"alpha".to_string()));
        assert!(index.dirs.contains(&"alpha/beta".to_string()));
        assert!(index.dirs.contains(&"gamma".to_string()));
        // root itself must not appear
        assert!(!index.dirs.contains(&"".to_string()));
    }

    #[test]
    fn query_returns_matches_for_pattern() {
        let dir = TempDir::new().unwrap();
        setup_tree(dir.path());

        let index = DestIndex::new(dir.path()).unwrap();
        let results = index.query("alp");
        assert!(!results.is_empty());
        assert!(results.iter().any(|r| r.contains("alpha")));
    }

    #[test]
    fn query_empty_pattern_returns_all_dirs() {
        let dir = TempDir::new().unwrap();
        setup_tree(dir.path());

        let index = DestIndex::new(dir.path()).unwrap();
        let results = index.query("");
        assert_eq!(results.len(), index.dirs.len());
    }

    #[test]
    fn rebuild_picks_up_new_directory() {
        let dir = TempDir::new().unwrap();
        setup_tree(dir.path());

        let mut index = DestIndex::new(dir.path()).unwrap();
        assert!(!index.dirs.contains(&"delta".to_string()));

        fs::create_dir(dir.path().join("delta")).unwrap();
        index.rebuild(dir.path()).unwrap();

        assert!(index.dirs.contains(&"delta".to_string()));
    }

    #[test]
    fn list_files_returns_sorted_filenames() {
        let dir = TempDir::new().unwrap();
        let sub = dir.path().join("sub");
        fs::create_dir(&sub).unwrap();
        fs::write(sub.join("c.txt"), b"c").unwrap();
        fs::write(sub.join("a.txt"), b"a").unwrap();
        fs::write(sub.join("b.txt"), b"b").unwrap();
        // A subdirectory should be excluded
        fs::create_dir(sub.join("nested")).unwrap();

        let files = DestIndex::list_files(dir.path(), "sub").unwrap();
        assert_eq!(files, vec!["a.txt", "b.txt", "c.txt"]);
    }

    #[test]
    fn list_files_errors_on_missing_path() {
        let dir = TempDir::new().unwrap();
        let result = DestIndex::list_files(dir.path(), "nonexistent");
        assert!(result.is_err());
    }
}
