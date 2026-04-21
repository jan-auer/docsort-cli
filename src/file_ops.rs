use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// Moves `src` into `dest_dir` with the filename `new_name`.
///
/// Creates `dest_dir` and any missing intermediate directories before moving.
/// Falls back to copy-then-delete if `fs::rename` fails (e.g. cross-filesystem).
/// Returns the final destination path.
pub fn move_file(src: &Path, dest_dir: &Path, new_name: &str) -> Result<PathBuf> {
    create_dir(dest_dir)?;

    let dest = dest_dir.join(new_name);

    match std::fs::rename(src, &dest) {
        Ok(()) => Ok(dest),
        Err(_) => {
            // Cross-filesystem move: copy then delete.
            std::fs::copy(src, &dest).with_context(|| {
                format!("failed to copy {} to {}", src.display(), dest.display())
            })?;
            std::fs::remove_file(src).with_context(|| {
                format!("failed to remove source file {} after copy", src.display())
            })?;
            Ok(dest)
        }
    }
}

/// Creates `path` and all missing intermediate directories.
///
/// Succeeds silently if the directory already exists.
fn create_dir(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path)
        .with_context(|| format!("failed to create directory: {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;

    use tempfile::TempDir;

    #[test]
    fn move_file_moves_to_existing_dest_dir() {
        let src_dir = TempDir::new().unwrap();
        let dest_dir = TempDir::new().unwrap();

        let src = src_dir.path().join("original.txt");
        fs::write(&src, b"content").unwrap();

        let result = move_file(&src, dest_dir.path(), "renamed.txt").unwrap();

        assert_eq!(result, dest_dir.path().join("renamed.txt"));
        assert!(result.exists());
        assert!(!src.exists());
        assert_eq!(fs::read(&result).unwrap(), b"content");
    }

    #[test]
    fn move_file_creates_intermediate_directories() {
        let src_dir = TempDir::new().unwrap();
        let base_dir = TempDir::new().unwrap();

        let src = src_dir.path().join("doc.pdf");
        fs::write(&src, b"pdf data").unwrap();

        let dest_dir = base_dir.path().join("a/b/c");
        let result = move_file(&src, &dest_dir, "doc.pdf").unwrap();

        assert!(result.exists());
        assert!(!src.exists());
    }

    #[test]
    fn move_file_errors_when_src_missing() {
        let dest_dir = TempDir::new().unwrap();
        let result = move_file(
            Path::new("/nonexistent/file.txt"),
            dest_dir.path(),
            "out.txt",
        );
        assert!(result.is_err());
    }

    #[test]
    fn create_dir_creates_nested_dirs() {
        let base = TempDir::new().unwrap();
        let nested = base.path().join("x/y/z");
        create_dir(&nested).unwrap();
        assert!(nested.is_dir());
    }

    #[test]
    fn create_dir_succeeds_if_already_exists() {
        let base = TempDir::new().unwrap();
        create_dir(base.path()).unwrap(); // already exists — must not error
    }
}
