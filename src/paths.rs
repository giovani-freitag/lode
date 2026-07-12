use std::env;
use std::path::{Path, PathBuf};

use anyhow::{bail, Result};

use crate::manifest::MANIFEST_FILENAME;

/// The resolved locations of a pack's files, discovered from the current directory upward.
#[derive(Debug)]
pub struct PackPaths {
    pub root: PathBuf,
}

impl PackPaths {
    /// Walk up from `start` looking for a `lode.jsonc`, so commands work from any subdirectory.
    pub fn discover(start: &Path) -> Result<PackPaths> {
        let mut dir = Some(start);
        while let Some(current) = dir {
            if current.join(MANIFEST_FILENAME).is_file() {
                return Ok(PackPaths {
                    root: current.to_path_buf(),
                });
            }
            dir = current.parent();
        }
        bail!(
            "no {MANIFEST_FILENAME} found in {} or any parent directory — run `lode init` first",
            start.display()
        );
    }

    /// Discover starting from the process's current working directory.
    pub fn discover_from_cwd() -> Result<PackPaths> {
        let cwd = env::current_dir()?;
        Self::discover(&cwd)
    }

    pub fn manifest(&self) -> PathBuf {
        self.root.join(MANIFEST_FILENAME)
    }

    pub fn lock(&self) -> PathBuf {
        self.root.join(crate::lock::LOCK_FILENAME)
    }

    /// The generated packwiz distribution tree (`pack/`).
    pub fn pack_dir(&self) -> PathBuf {
        self.root.join("pack")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn discover_finds_manifest_in_the_start_directory_itself() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        fs::write(root.join(MANIFEST_FILENAME), "{}").unwrap();

        let paths = PackPaths::discover(root).unwrap();

        assert_eq!(paths.root, root);
    }

    #[test]
    fn discover_walks_up_from_a_nested_subdirectory() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        fs::write(root.join(MANIFEST_FILENAME), "{}").unwrap();
        let nested = root.join("pack").join("kubejs").join("server_scripts");
        fs::create_dir_all(&nested).unwrap();

        let paths = PackPaths::discover(&nested).unwrap();

        assert_eq!(paths.root, root);
    }

    #[test]
    fn discover_returns_the_nearest_manifest_not_an_outer_one() {
        let tmp = tempdir().unwrap();
        let outer = tmp.path();
        fs::write(outer.join(MANIFEST_FILENAME), "{}").unwrap();
        let inner = outer.join("sub-pack");
        let nested = inner.join("a").join("b");
        fs::create_dir_all(&nested).unwrap();
        fs::write(inner.join(MANIFEST_FILENAME), "{}").unwrap();

        let paths = PackPaths::discover(&nested).unwrap();

        assert_eq!(paths.root, inner);
    }

    #[test]
    fn discover_errors_when_no_manifest_exists_in_any_ancestor() {
        let tmp = tempdir().unwrap();
        let nested = tmp.path().join("a").join("b");
        fs::create_dir_all(&nested).unwrap();

        let err = PackPaths::discover(&nested).unwrap_err();

        let msg = format!("{err:#}");
        assert!(msg.contains(MANIFEST_FILENAME), "{msg}");
        assert!(msg.contains("lode init"), "{msg}");
    }

    #[test]
    fn manifest_accessor_joins_the_manifest_filename_onto_root() {
        let paths = PackPaths {
            root: PathBuf::from("/tmp/pack"),
        };

        assert_eq!(
            paths.manifest(),
            PathBuf::from("/tmp/pack").join(MANIFEST_FILENAME)
        );
    }

    #[test]
    fn lock_accessor_joins_the_lock_filename_onto_root() {
        let paths = PackPaths {
            root: PathBuf::from("/tmp/pack"),
        };

        assert_eq!(
            paths.lock(),
            PathBuf::from("/tmp/pack").join(crate::lock::LOCK_FILENAME)
        );
    }

    #[test]
    fn pack_dir_accessor_points_at_the_generated_distribution_tree() {
        let paths = PackPaths {
            root: PathBuf::from("/tmp/pack"),
        };

        assert_eq!(paths.pack_dir(), PathBuf::from("/tmp/pack").join("pack"));
    }

    #[test]
    fn accessors_all_hang_off_the_discovered_root() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        fs::write(root.join(MANIFEST_FILENAME), "{}").unwrap();

        let paths = PackPaths::discover(root).unwrap();

        assert_eq!(paths.manifest(), root.join(MANIFEST_FILENAME));
        assert_eq!(paths.lock(), root.join(crate::lock::LOCK_FILENAME));
        assert_eq!(paths.pack_dir(), root.join("pack"));
    }
}
