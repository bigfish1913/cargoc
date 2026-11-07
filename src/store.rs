use crate::{
    Result,
    config::Config,
    fail,
    util::{GIB, absolute, atomic_write, check_path, identity_path, now, same_or_descendant},
};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
};

const MARKER: &str = ".cargoc-root.json";

fn empty_path(path: &Path) -> bool {
    path.as_os_str().is_empty()
}

pub struct Lease(File);
impl Lease {
    pub fn acquire(path: &Path, create: bool) -> Result<Option<Self>> {
        check_path(path)?;
        if create {
            fs::create_dir_all(path.parent().ok_or("Lock has no parent")?)?;
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(create)
            .truncate(false)
            .open(path)?;
        match file.try_lock_exclusive() {
            Ok(()) => Ok(Some(Self(file))),
            Err(_) => Ok(None),
        }
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.0);
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Marker {
    version: u32,
    root_id: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CacheKind {
    #[default]
    Workspace,
    Install,
}

impl CacheKind {
    fn is_workspace(&self) -> bool {
        *self == Self::Workspace
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Record {
    pub root_id: String,
    pub key: String,
    #[serde(default, skip_serializing_if = "empty_path")]
    pub workspace: PathBuf,
    #[serde(default, skip_serializing_if = "CacheKind::is_workspace")]
    pub kind: CacheKind,
    /// Only a digest is persisted: install URLs/arguments may contain credentials.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub install_identity: Option<String>,
    pub last_used: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct Entry {
    pub record: Record,
    pub bytes: u64,
}

#[derive(Default)]
pub struct GcOptions {
    pub all_idle: bool,
    pub dry_run: bool,
    pub protected: HashSet<String>,
}

#[derive(Default, Debug, Serialize)]
pub struct GcReport {
    pub dry_run: bool,
    pub busy: bool,
    pub before_bytes: u64,
    pub after_bytes: u64,
    pub removed_bytes: u64,
    pub free_bytes: u64,
    pub unresolved: bool,
    pub removed: Vec<String>,
    pub skipped: Vec<String>,
}

#[derive(Clone)]
pub struct Store {
    pub config: Config,
    pub root: PathBuf,
    root_id: String,
}

#[derive(Default)]
struct Tree {
    files: Vec<(PathBuf, u64)>,
    dirs: Vec<PathBuf>,
    bytes: u64,
}

fn scan(path: &Path) -> Result<Tree> {
    check_path(path)?;
    if !path.exists() {
        return Ok(Tree::default());
    }
    let mut tree = Tree::default();
    let mut pending = vec![path.to_path_buf()];
    while let Some(dir) = pending.pop() {
        check_path(&dir)?;
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e.into()),
        };
        tree.dirs.push(dir);
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            check_path(&path)?;
            let meta = match fs::symlink_metadata(&path) {
                Ok(meta) => meta,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e.into()),
            };
            if meta.is_dir() {
                pending.push(path);
            } else if meta.is_file() {
                tree.bytes = tree.bytes.saturating_add(meta.len());
                tree.files.push((path, meta.len()));
            } else {
                return fail(format!("Non-regular cache file: {}", path.display()));
            }
        }
    }
    Ok(tree)
}

fn clear(tree: &Tree) -> Result<u64> {
    let mut removed: u64 = 0;
    for (path, size) in &tree.files {
        if path.file_name().is_some_and(|name| name == ".cargo-lock") {
            continue;
        }
        check_path(path)?;
        match fs::remove_file(path) {
            Ok(()) => removed = removed.saturating_add(*size),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                #[cfg(windows)]
                if e.kind() == std::io::ErrorKind::PermissionDenied {
                    let mut permissions = fs::metadata(path)?.permissions();
                    // Windows-only: clear the DOS read-only flag, not Unix mode bits.
                    #[allow(clippy::permissions_set_readonly_false)]
                    permissions.set_readonly(false);
                    fs::set_permissions(path, permissions)?;
                    fs::remove_file(path)?;
                    removed = removed.saturating_add(*size);
                    continue;
                }
                return Err(e.into());
            }
        }
    }
    // Preserve every native lock and its parent directory, never replace its inode.
    for dir in tree.dirs.iter().rev() {
        check_path(dir)?;
        let _ = fs::remove_dir(dir);
    }
    Ok(removed)
}

impl Store {
    pub fn open(config: Config, create: bool) -> Result<Self> {
        config.validate()?;
        let root = absolute(&config.cache_dir)?;
        check_path(&root)?;
        let marker_path = root.join(MARKER);
        check_path(&marker_path)?;
        if !marker_path.exists() {
            if !create {
                return fail("Uninitialized cache root; run cargoc init");
            }
            let lock_path = root.parent().ok_or("Cache has no parent")?.join(format!(
                ".{}.cargoc-init.lock",
                root.file_name()
                    .ok_or("Cache has no name")?
                    .to_string_lossy()
            ));
            let _init = Lease::acquire(&lock_path, true)?
                .ok_or("Another process is initializing the cache")?;
            if !marker_path.exists() {
                if root.exists() && fs::read_dir(&root)?.next().is_some() {
                    return fail(format!(
                        "Refusing to adopt nonempty/unowned directory: {}",
                        root.display()
                    ));
                }
                fs::create_dir_all(&root)?;
                let root_id = format!(
                    "{:x}",
                    Sha256::digest(format!(
                        "{}:{}:{}",
                        root.display(),
                        now(),
                        std::process::id()
                    ))
                );
                atomic_write(
                    &marker_path,
                    &serde_json::to_vec(&Marker {
                        version: 1,
                        root_id,
                    })?,
                )?;
            }
        }
        let marker: Marker = serde_json::from_slice(&fs::read(marker_path)?)?;
        if marker.version != 1 || !valid_key(&marker.root_id) {
            return fail("Invalid cache ownership marker");
        }
        for dir in ["workspaces", "state", "locks"] {
            check_path(&root.join(dir))?;
            fs::create_dir_all(root.join(dir))?;
        }
        Ok(Self {
            config,
            root,
            root_id: marker.root_id,
        })
    }

    pub fn key(workspace: &Path) -> Result<String> {
        let value = identity_path(workspace)?;
        Ok(format!("{:x}", Sha256::digest(value.as_bytes())))
    }

    pub fn workspace(&self, workspace: &Path) -> Result<(String, PathBuf)> {
        let workspace = absolute(workspace)?;
        if same_or_descendant(&workspace, &self.root)? {
            return fail("Cache root cannot contain the source workspace");
        }
        let key = Self::key(&workspace)?;
        let path = self.root.join("workspaces").join(&key);
        check_path(&path)?;
        Ok((key, path))
    }

    pub fn lease(&self, key: &str) -> Result<Option<Lease>> {
        if !valid_key(key) {
            return fail("Invalid workspace key");
        }
        Lease::acquire(&self.root.join("locks").join(format!("{key}.lock")), true)
    }

    /// Caller must hold this workspace's lease.
    pub fn touch(&self, workspace: &Path) -> Result<(String, PathBuf)> {
        let (key, path) = self.workspace(workspace)?;
        fs::create_dir_all(&path)?;
        let record = Record {
            root_id: self.root_id.clone(),
            key: key.clone(),
            workspace: absolute(workspace)?,
            kind: CacheKind::Workspace,
            install_identity: None,
            last_used: now(),
        };
        atomic_write(
            &self.root.join("state").join(format!("{key}.json")),
            &serde_json::to_vec(&record)?,
        )?;
        Ok((key, path))
    }

    /// Installation caches share the same physical layout and GC/lease namespace,
    /// but never pretend to be a source workspace.
    pub fn install(&self, identity: &str) -> Result<(String, PathBuf)> {
        if !valid_key(identity) {
            return fail("Invalid install identity");
        }
        let key = format!(
            "{:x}",
            Sha256::digest(format!("cargoc-install-v1:{identity}"))
        );
        let path = self.root.join("workspaces").join(&key);
        check_path(&path)?;
        Ok((key, path))
    }

    /// Caller must hold the installation's lease.
    pub fn touch_install(&self, identity: &str) -> Result<(String, PathBuf)> {
        let (key, path) = self.install(identity)?;
        fs::create_dir_all(&path)?;
        let record = Record {
            root_id: self.root_id.clone(),
            key: key.clone(),
            workspace: PathBuf::new(),
            kind: CacheKind::Install,
            install_identity: Some(identity.to_owned()),
            last_used: now(),
        };
        atomic_write(
            &self.root.join("state").join(format!("{key}.json")),
            &serde_json::to_vec(&record)?,
        )?;
        Ok((key, path))
    }

    fn read_record(&self, key: &str) -> Result<Record> {
        if !valid_key(key) {
            return fail("Invalid state filename");
        }
        let path = self.root.join("state").join(format!("{key}.json"));
        check_path(&path)?;
        let record: Record = serde_json::from_slice(&fs::read(path)?)?;
        if record.root_id != self.root_id
            || record.key != key
            || !record.last_used.is_finite()
            || record.last_used < 0.0
        {
            return fail("Invalid cache record ownership/identity");
        }
        match record.kind {
            CacheKind::Workspace => {
                if record.install_identity.is_some()
                    || !record.workspace.is_absolute()
                    || self.workspace(&record.workspace)?.0 != key
                {
                    return fail("Invalid workspace cache identity");
                }
            }
            CacheKind::Install => {
                if !record.workspace.as_os_str().is_empty()
                    || self
                        .install(
                            record
                                .install_identity
                                .as_deref()
                                .ok_or("Missing install identity")?,
                        )?
                        .0
                        != key
                {
                    return fail("Invalid install cache identity");
                }
            }
        }
        Ok(record)
    }

    pub fn entries(&self) -> Result<Vec<Entry>> {
        check_path(&self.root.join("state"))?;
        let mut entries = Vec::new();
        for file in fs::read_dir(self.root.join("state"))? {
            let path = file?.path();
            if path.extension().is_some_and(|e| e == "json") {
                let key = path
                    .file_stem()
                    .ok_or("Invalid state filename")?
                    .to_string_lossy();
                let record = self.read_record(&key)?;
                entries.push(Entry {
                    bytes: scan(&self.root.join("workspaces").join(&record.key))?.bytes,
                    record,
                });
            }
        }
        Ok(entries)
    }

    pub fn free(&self) -> Result<u64> {
        Ok(fs2::available_space(&self.root)?)
    }

    pub fn collect(&self, options: &GcOptions) -> Result<GcReport> {
        self.collect_with_free(options, || self.free())
    }

    /// Disk-free injection also lets tests exercise pressure without filling a disk.
    pub fn collect_with_free(
        &self,
        options: &GcOptions,
        free: impl Fn() -> Result<u64>,
    ) -> Result<GcReport> {
        let mut report = GcReport {
            dry_run: options.dry_run,
            ..GcReport::default()
        };
        let Some(_gc) = Lease::acquire(&self.root.join("locks/gc.lock"), true)? else {
            report.busy = true;
            return Ok(report);
        };
        let mut entries = self.entries()?;
        entries.sort_by(|a, b| a.record.last_used.total_cmp(&b.record.last_used));
        let mut total = entries.iter().map(|e| e.bytes).sum::<u64>();
        report.before_bytes = total;
        let mut available = free()?;
        let cfg = &self.config;
        let capacity_pressure = total as f64 > cfg.max_cache_gib * GIB;
        let target = cfg.max_cache_gib * cfg.low_water_ratio * GIB;
        for entry in entries {
            if entry.bytes == 0 {
                continue;
            }
            let key = entry.record.key;
            let stale = cfg.max_age_days > 0.0
                && now() - entry.record.last_used > cfg.max_age_days * 86400.0;
            // A healthy disk makes the capacity limit a soft limit: evict only
            // stale entries, otherwise alternating workspaces can churn their
            // dependency caches at every build. Disk pressure still overrides
            // the age floor so the safety threshold can be recovered.
            let disk_pressure = (available as f64) < cfg.min_free_gib * GIB;
            let eligible = disk_pressure || stale || (capacity_pressure && cfg.max_age_days == 0.0);
            if !options.all_idle
                && (!eligible || (!disk_pressure && capacity_pressure && total as f64 <= target))
            {
                continue;
            }
            if options.protected.contains(&key) {
                report.skipped.push(format!("{key}: current workspace"));
                continue;
            }
            let candidate = (|| -> Result<u64> {
                let _lease = self.lease(&key)?.ok_or("workspace is active/locked")?;
                let record = self.read_record(&key)?;
                let stale =
                    cfg.max_age_days > 0.0 && now() - record.last_used > cfg.max_age_days * 86400.0;
                let disk_pressure = (available as f64) < cfg.min_free_gib * GIB;
                let eligible =
                    disk_pressure || stale || (capacity_pressure && cfg.max_age_days == 0.0);
                if !options.all_idle
                    && (!eligible
                        || (!disk_pressure && capacity_pressure && total as f64 <= target))
                {
                    return Ok(0);
                }
                let path = self.root.join("workspaces").join(&key);
                let tree = scan(&path)?;
                let mut native_locks = Vec::new();
                for (path, _) in &tree.files {
                    if path.file_name().is_some_and(|name| name == ".cargo-lock") {
                        native_locks
                            .push(Lease::acquire(path, false)?.ok_or("Cargo native lock is busy")?);
                    }
                }
                // Validate once more after all native locks have been obtained.
                let tree = scan(&path)?;
                if options.dry_run {
                    Ok(tree
                        .files
                        .iter()
                        .filter(|(p, _)| p.file_name().is_none_or(|n| n != ".cargo-lock"))
                        .map(|(_, n)| n)
                        .sum())
                } else {
                    match clear(&tree) {
                        Ok(removed) => Ok(removed),
                        Err(error) => {
                            // Account for successful unlinks before an I/O error, while
                            // the workspace/native leases still prevent concurrent builds.
                            let remaining = scan(&path)?.bytes;
                            report
                                .skipped
                                .push(format!("{key}: partial cleanup: {error}"));
                            Ok(tree.bytes.saturating_sub(remaining))
                        }
                    }
                }
            })();
            match candidate {
                Ok(removed) => {
                    total = total.saturating_sub(removed);
                    available = if options.dry_run {
                        available.saturating_add(removed)
                    } else {
                        free()?
                    };
                    report.removed_bytes = report.removed_bytes.saturating_add(removed);
                    if removed > 0 {
                        report.removed.push(key);
                    }
                }
                Err(error) => report.skipped.push(format!("{key}: {error}")),
            }
        }
        report.after_bytes = total;
        report.free_bytes = available;
        report.unresolved =
            total as f64 > cfg.max_cache_gib * GIB || (available as f64) < cfg.min_free_gib * GIB;
        Ok(report)
    }
}

fn valid_key(key: &str) -> bool {
    key.len() == 64
        && key
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}
