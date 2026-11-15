//! Filesystem facts, behind a trait so tests and `--state` use fixtures.
//!
//! The ownership reconciler ports `nixstorage`'s `reconcile.sh`: it only ever
//! reads ownership and mode, and the plan it builds is a set of `chown -h` and
//! `chmod` calls on the declared paths. Absent paths are reported, never
//! created.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// Ownership and mode of a path, as `lstat` reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileMeta {
    /// The owning uid.
    pub uid: u32,
    /// The owning gid.
    pub gid: u32,
    /// The mode bits, including setuid/setgid/sticky.
    pub mode: u32,
    /// Whether the path is a directory (a symlink is not followed).
    pub is_dir: bool,
}

impl FileMeta {
    /// The permission bits without the file-type bits.
    #[must_use]
    pub fn permissions(&self) -> u32 {
        self.mode & 0o7777
    }
}

/// Read-only filesystem facts.
pub trait Fs {
    /// Metadata of `path` itself (never following a symlink), or `None` when
    /// the path is absent.
    fn meta(&self, path: &Path) -> Result<Option<FileMeta>>;

    /// Every path at or beneath `root` not owned by `uid:gid`, with pruned
    /// subtrees excluded.
    ///
    /// This is the mis-owned-only walk from `reconcile.sh`: a converged tree
    /// costs one walk and no `chown` calls.
    fn mis_owned(&self, root: &Path, uid: u32, gid: u32, prune: &[PathBuf])
    -> Result<Vec<PathBuf>>;
}

/// The real filesystem, through `lstat`.
#[derive(Debug, Clone, Copy, Default)]
pub struct RealFs;

impl Fs for RealFs {
    fn meta(&self, path: &Path) -> Result<Option<FileMeta>> {
        lstat(path)
    }

    fn mis_owned(
        &self,
        root: &Path,
        uid: u32,
        gid: u32,
        prune: &[PathBuf],
    ) -> Result<Vec<PathBuf>> {
        let mut found = Vec::new();
        walk(root, uid, gid, prune, &mut found, true)?;
        found.sort();
        Ok(found)
    }
}

fn lstat(path: &Path) -> Result<Option<FileMeta>> {
    use std::os::unix::fs::MetadataExt;

    match std::fs::symlink_metadata(path) {
        Ok(metadata) => Ok(Some(FileMeta {
            uid: metadata.uid(),
            gid: metadata.gid(),
            mode: metadata.mode(),
            is_dir: metadata.file_type().is_dir(),
        })),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(Error::io(path, error)),
    }
}

fn walk(
    path: &Path,
    uid: u32,
    gid: u32,
    prune: &[PathBuf],
    found: &mut Vec<PathBuf>,
    is_root: bool,
) -> Result<()> {
    let Some(metadata) = lstat(path)? else {
        return Ok(());
    };
    if !is_root && is_pruned(path, prune) {
        return Ok(());
    }
    if metadata.uid != uid || metadata.gid != gid {
        found.push(path.to_path_buf());
    }
    if metadata.is_dir {
        let entries = std::fs::read_dir(path).map_err(|error| Error::io(path, error))?;
        for entry in entries {
            let entry = entry.map_err(|error| Error::io(path, error))?;
            walk(&entry.path(), uid, gid, prune, found, false)?;
        }
    }
    Ok(())
}

fn is_pruned(path: &Path, prune: &[PathBuf]) -> bool {
    prune.iter().any(|candidate| path.starts_with(candidate))
}
