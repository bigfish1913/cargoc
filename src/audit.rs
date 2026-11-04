//! Read-only inventory of an explicitly selected, non-managed cache directory.
//! An available Cargo lock is a point-in-time observation, never deletion consent.
use crate::{
    Result, fail,
    util::{absolute, check_path, same_or_descendant},
};
use fs2::FileExt;
use serde::Serialize;
use std::{
    fs::{self, OpenOptions},
    path::{Path, PathBuf},
};

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum LockState {
    Available,
    Busy,
    Unavailable,
}

#[derive(Debug, Serialize)]
pub struct LockReport {
    pub path: PathBuf,
    pub state: LockState,
}

#[derive(Debug, Serialize)]
pub struct AuditReport {
    pub path: PathBuf,
    pub read_only: bool,
    pub bytes: u64,
    pub files: u64,
    pub directories: u64,
    pub locks: Vec<LockReport>,
    pub warnings: Vec<String>,
}

fn lock_state(path: &Path) -> LockState {
    // Never create a lock: an audit must not change the directory being audited.
    let Ok(file) = OpenOptions::new().read(true).write(true).open(path) else {
        return LockState::Unavailable;
    };
    match file.try_lock_exclusive() {
        Ok(()) => {
            let _ = FileExt::unlock(&file);
            LockState::Available
        }
        Err(error)
            if error.kind() == std::io::ErrorKind::WouldBlock
                || error.raw_os_error() == fs2::lock_contended_error().raw_os_error() =>
        {
            LockState::Busy
        }
        Err(_) => LockState::Unavailable,
    }
}

pub fn inspect(path: &Path, managed_root: &Path) -> Result<AuditReport> {
    let path = absolute(path)?;
    check_path(&path)?;
    if same_or_descendant(&path, managed_root)? || same_or_descendant(managed_root, &path)? {
        return fail("Audit path overlaps the managed cache root; use cargoc status/clean instead");
    }
    if !path.is_dir() {
        return fail("Audit needs an existing non-managed directory");
    }
    let mut report = AuditReport {
        path: path.clone(),
        read_only: true,
        bytes: 0,
        files: 0,
        directories: 0,
        locks: Vec::new(),
        warnings: vec![
            "Inventory only: unknown files are not adopted, moved or deleted.".into(),
            "Lock observations are momentary. Available/missing locks do not prove a directory is idle or safe to delete.".into(),
            "Final artifacts may be present. Stop all producers and verify the original build-dir/target-dir configuration before any separate cleanup.".into(),
        ],
    };
    let mut pending = vec![path];
    while let Some(dir) = pending.pop() {
        check_path(&dir)?;
        report.directories += 1;
        for entry in fs::read_dir(dir)? {
            let path = entry?.path();
            check_path(&path)?;
            let metadata = fs::symlink_metadata(&path)?;
            if metadata.is_dir() {
                pending.push(path);
            } else if metadata.is_file() {
                if path
                    .file_name()
                    .is_some_and(|name| name == ".cargoc-root.json")
                {
                    return fail(
                        "Found a cargoc ownership marker; refusing to treat managed data as external",
                    );
                }
                report.bytes = report.bytes.saturating_add(metadata.len());
                report.files += 1;
                if path.file_name().is_some_and(|name| name == ".cargo-lock") {
                    report.locks.push(LockReport {
                        state: lock_state(&path),
                        path,
                    });
                }
            } else {
                return fail("Refusing non-regular entries in audit directory");
            }
        }
    }
    report.locks.sort_by(|a, b| a.path.cmp(&b.path));
    if report.locks.is_empty() {
        report.warnings.push(
            "No existing .cargo-lock files were found; no activity assessment is possible.".into(),
        );
    }
    Ok(report)
}
