//! Private file handling: 0700 directories, 0600 files, atomic replace.

use std::fs::{self, DirBuilder, File, OpenOptions, Permissions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;

use crate::crypto::fill_random;
use crate::error::{Result, io};

/// Creates `dir` (and parents) as 0700 and tightens `dir` itself to 0700.
pub(crate) fn ensure_private_dir(dir: &Path) -> Result<()> {
    DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .map_err(io(format!("create {}", dir.display())))?;
    fs::set_permissions(dir, Permissions::from_mode(0o700))
        .map_err(io(format!("chmod 0700 {}", dir.display())))
}

/// Writes `data` to `path` atomically: a 0600 temp file in the same
/// directory, fsync, rename over `path`, fsync the directory.
pub(crate) fn write_atomic(path: &Path, data: &[u8]) -> Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    let mut rnd = [0u8; 8];
    fill_random(&mut rnd)?;
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = dir.join(format!(".{file_name}.tmp-{}", hex::encode(rnd)));
    let result = (|| {
        let mut f = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)
            .map_err(io(format!("create {}", tmp.display())))?;
        f.write_all(data)
            .map_err(io(format!("write {}", tmp.display())))?;
        f.sync_all()
            .map_err(io(format!("fsync {}", tmp.display())))?;
        drop(f);
        fs::rename(&tmp, path).map_err(io(format!("replace {}", path.display())))?;
        File::open(dir)
            .and_then(|d| d.sync_all())
            .map_err(io(format!("fsync {}", dir.display())))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// Holds an exclusive advisory lock on `<dir>/vault.lock` until dropped, so
/// concurrent writers serialise their read-modify-write.
pub(crate) struct DirLock {
    _file: File,
}

impl DirLock {
    pub(crate) fn acquire(dir: &Path) -> Result<Self> {
        let path = dir.join("vault.lock");
        let f = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(&path)
            .map_err(io(format!("open {}", path.display())))?;
        f.lock().map_err(io(format!("lock {}", path.display())))?;
        Ok(Self { _file: f })
    }
}
