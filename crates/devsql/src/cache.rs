//! Where derived indexes (Codex, Claude tool calls, Grok) are stored.

use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// The cache file for the index of kind `kind` built from `source`.
///
/// `DEVSQL_CACHE_DIR` wins when set. A source inside the system temporary
/// directory, such as a test fixture, caches there too, so indexes of
/// throwaway data never accumulate in the user's cache directory. Everything
/// else caches under the user cache directory.
pub(crate) fn cache_path(kind: &str, source: &Path) -> PathBuf {
    let canonical = source
        .canonicalize()
        .unwrap_or_else(|_| source.to_path_buf());
    let digest = Sha256::digest(canonical.to_string_lossy().as_bytes());
    cache_root(&canonical)
        .join(kind)
        .join(format!("{digest:x}.sqlite"))
}

fn cache_root(canonical_source: &Path) -> PathBuf {
    if let Some(dir) = std::env::var_os("DEVSQL_CACHE_DIR").filter(|dir| !dir.is_empty()) {
        return PathBuf::from(dir);
    }
    let temp = std::env::temp_dir();
    let temp_canonical = temp.canonicalize().unwrap_or_else(|_| temp.clone());
    if canonical_source.starts_with(&temp_canonical) || canonical_source.starts_with(&temp) {
        return temp.join("devsql-cache");
    }
    dirs::cache_dir()
        .map(|dir| dir.join("devsql"))
        .unwrap_or_else(|| temp.join("devsql-cache"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temporary_sources_cache_in_the_temporary_directory() {
        if std::env::var_os("DEVSQL_CACHE_DIR").is_some() {
            return;
        }
        let source = tempfile::tempdir().unwrap();
        let path = cache_path("codex-index", source.path());
        let temp = std::env::temp_dir();
        let temp = temp.canonicalize().unwrap_or(temp);
        let path = path.canonicalize().unwrap_or(path);
        assert!(
            path.starts_with(&temp) || path.starts_with(std::env::temp_dir()),
            "{} is outside {}",
            path.display(),
            temp.display()
        );
        assert!(path.ends_with(
            Path::new("codex-index").join(path.file_name().unwrap())
        ));
    }

    #[test]
    fn the_same_source_always_maps_to_the_same_cache() {
        let source = tempfile::tempdir().unwrap();
        assert_eq!(
            cache_path("grok-index", source.path()),
            cache_path("grok-index", source.path())
        );
        assert_ne!(
            cache_path("grok-index", source.path()),
            cache_path("codex-index", source.path())
        );
    }
}
