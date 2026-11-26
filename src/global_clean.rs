//! Explicit, bounded cleanup of Cargo target directories. Never called by automatic GC.
//! Existing locks are only observations; users must stop all producers before execution.
use crate::{
    Result, cargo,
    config::{Config, home},
    fail,
    process::OwnedChild,
    runner,
    store::{GcOptions, GcReport, Lease, Store, existing_lock, is_native_lock},
    util::{absolute, check_path, format_bytes, read_bounded, same_or_descendant},
};
use serde::Serialize;
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

#[derive(Default)]
pub struct Options {
    pub roots: Vec<PathBuf>,
    pub include_targets: bool,
    pub dry_run: bool,
    pub yes: bool,
    pub timeout_seconds: u64,
}

#[derive(Serialize)]
pub struct TargetReport {
    pub workspace: PathBuf,
    pub target: PathBuf,
    pub before_bytes: u64,
    pub before: String,
    pub removed_bytes: u64,
    pub removed: String,
    pub deletes_final_artifacts: bool,
    pub status: String,
    pub reason: Option<String>,
    #[serde(skip)]
    identity: String,
}

#[derive(Serialize)]
pub struct VolumeReport {
    pub path: PathBuf,
    pub device: String,
    pub free_before_bytes: u64,
    pub free_after_bytes: u64,
}

#[derive(Default, Serialize)]
pub struct Report {
    pub dry_run: bool,
    pub managed_report: Option<GcReport>,
    pub managed_error: Option<String>,
    pub targets: Vec<TargetReport>,
    pub skipped: Vec<String>,
    pub volumes: Vec<VolumeReport>,
    pub incomplete: bool,
}

#[derive(Default)]
struct Tree {
    bytes: u64,
    locks: Vec<PathBuf>,
}

fn device(path: &Path) -> Result<String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(fs::metadata(path)?.dev().to_string())
    }
    #[cfg(not(unix))]
    {
        // Windows mount points/junctions are rejected by check_path.
        Ok(fs::canonicalize(path)?
            .components()
            .next()
            .ok_or("Path has no volume")?
            .as_os_str()
            .to_string_lossy()
            .to_string())
    }
}

fn identity(path: &Path) -> Result<String> {
    let metadata = fs::metadata(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(format!("{}:{}", metadata.dev(), metadata.ino()))
    }
    #[cfg(not(unix))]
    {
        Ok(format!(
            "{}:{:?}",
            fs::canonicalize(path)?.display(),
            metadata.created()?
        ))
    }
}

fn overlaps(a: &Path, b: &Path) -> Result<bool> {
    Ok(same_or_descendant(a, b)? || same_or_descendant(b, a)?)
}

/// Conservative inventory: reject links, special files, nested mounts, source manifests
/// and ownership markers. Authorization is for the *whole* target, not just known files.
fn tree(root: &Path) -> Result<Tree> {
    check_path(root)?;
    if !root.try_exists()? {
        return Ok(Tree::default());
    }
    if !root.is_dir() {
        return fail("Target is not a directory");
    }
    if !read_bounded(&root.join("CACHEDIR.TAG"), 4096)?
        .starts_with(b"Signature: 8a477f597d28d172789f06886806bc55")
    {
        return fail("Missing or invalid Cargo CACHEDIR.TAG; refusing to adopt target");
    }
    let dev = device(root)?;
    let mut result = Tree::default();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        check_path(&dir)?;
        if device(&dir)? != dev {
            return fail("Nested mount in target; refusing to cross it");
        }
        for entry in fs::read_dir(&dir)? {
            let path = entry?.path();
            check_path(&path)?;
            let meta = fs::symlink_metadata(&path)?;
            let name = path
                .file_name()
                .ok_or("Entry has no name")?
                .to_string_lossy();
            if matches!(
                name.to_ascii_lowercase().as_str(),
                "cargo.toml" | ".git" | ".cargoc-root.json" | ".cargo"
            ) {
                return fail(format!(
                    "Source/configuration/ownership marker inside target: {}",
                    path.display()
                ));
            }
            if meta.is_dir() {
                // Root-level directory names must be a known layout or a target triple.
                if dir == root
                    && !matches!(name.as_ref(), "debug" | "release" | "tmp" | "doc")
                    && !(name.contains('-') && path.join("debug").is_dir())
                    && !(name.contains('-') && path.join("release").is_dir())
                {
                    return fail(format!("Unknown target layout: {}", path.display()));
                }
                pending.push(path);
            } else if meta.is_file() {
                if dir == root
                    && !is_native_lock(&path)
                    && !matches!(
                        name.as_ref(),
                        "CACHEDIR.TAG"
                            | ".rustc_info.json"
                            | ".future-incompat-report.json"
                            | ".rustdoc_fingerprint.json"
                    )
                {
                    return fail(format!("Unknown file in target root: {}", path.display()));
                }
                result.bytes = result.bytes.saturating_add(meta.len());
                if is_native_lock(&path) {
                    result.locks.push(path);
                }
            } else {
                return fail(format!("Non-regular file: {}", path.display()));
            }
        }
    }
    Ok(result)
}

fn locks(paths: &[PathBuf]) -> Result<Vec<Lease>> {
    let mut held = Vec::new();
    for path in paths {
        if let Some(lease) = existing_lock(path)? {
            held.push(lease);
        }
    }
    Ok(held)
}

/// Use files rather than pipes so verbose children cannot deadlock on pipe capacity.
/// Neither metadata nor clean is allowed to wait indefinitely for native Cargo locks.
fn run(command: &mut Command, timeout: u64) -> Result<Vec<u8>> {
    let temp = fs::canonicalize(std::env::temp_dir())?;
    let out = tempfile::NamedTempFile::new_in(&temp)?;
    let err = tempfile::NamedTempFile::new_in(&temp)?;
    command
        .stdout(Stdio::from(out.reopen()?))
        .stderr(Stdio::from(err.reopen()?))
        .stdin(Stdio::null());
    let mut child = OwnedChild::spawn(command)?;
    let start = Instant::now();
    loop {
        if runner::interrupted() {
            child.stop()?;
            return fail("Global cleanup interrupted");
        }
        if let Some(status) = child.status()? {
            if !status.success() {
                let stderr = read_bounded(err.path(), 1024 * 1024).unwrap_or_default();
                return fail(format!(
                    "Cargo exited with {status}: {}",
                    String::from_utf8_lossy(&stderr)
                        .chars()
                        .take(2000)
                        .collect::<String>()
                ));
            }
            return read_bounded(out.path(), 4 * 1024 * 1024);
        }
        if start.elapsed() >= Duration::from_secs(timeout) {
            child.stop()?;
            return fail(
                "Cargo timed out (possibly waiting for a busy native lock); no force deletion attempted",
            );
        }
        thread::sleep(Duration::from_millis(25));
    }
}

fn output_config(target: &Path) -> Result<String> {
    let path = target.to_str().ok_or("Target path is not UTF-8")?;
    if path.contains(['{', '}']) {
        return fail(
            "Target contains Cargo template braces; refusing a potentially expanded deletion path",
        );
    }
    Ok(format!(
        "build.build-dir={}",
        toml::Value::String(path.into())
    ))
}

fn command(cargo: &Path, workspace: &Path, target: &Path) -> Result<Command> {
    let mut command = cargo::command(cargo)?;
    command
        .current_dir(workspace)
        .arg("--config")
        .arg(output_config(target)?)
        .env("CARGO_TARGET_DIR", target)
        .env("CARGO_BUILD_TARGET_DIR", target)
        .env("CARGO_BUILD_BUILD_DIR", target)
        .env("RUSTUP_AUTO_INSTALL", "0")
        .env("CARGO_NET_OFFLINE", "true");
    Ok(command)
}

fn check_version(cargo: &Path, dir: &Path, timeout: u64) -> Result<()> {
    let mut version = cargo::command(cargo)?;
    version
        .current_dir(dir)
        .arg("--version")
        .env("RUSTUP_AUTO_INSTALL", "0");
    let text = String::from_utf8(run(&mut version, timeout)?)?;
    if !cargo::version_number(&text).is_some_and(|v| v >= (1, 91)) {
        return fail("Global target cleanup requires real Cargo 1.91+ in each workspace");
    }
    Ok(())
}

fn workspace(cargo: &Path, dir: &Path, timeout: u64) -> Result<PathBuf> {
    // Rustup may select a different toolchain in each workspace.
    check_version(cargo, dir, timeout)?;
    let target = dir.join("target");
    let mut command = command(cargo, dir, &target)?;
    command
        .args([
            "metadata",
            "--no-deps",
            "--offline",
            "--locked",
            "--format-version",
            "1",
            "--manifest-path",
        ])
        .arg(dir.join("Cargo.toml"));
    let data: serde_json::Value = serde_json::from_slice(&run(&mut command, timeout)?)?;
    let workspace = PathBuf::from(
        data.get("workspace_root")
            .and_then(|v| v.as_str())
            .ok_or("Cargo metadata has no workspace_root")?,
    );
    check_path(&workspace)?;
    Ok(fs::canonicalize(workspace)?)
}

fn authorized_root<'a>(path: &Path, roots: &'a [PathBuf]) -> Option<&'a PathBuf> {
    roots
        .iter()
        .filter(|root| path.starts_with(root))
        .max_by_key(|root| root.components().count())
}

fn validate_target(
    config: &Config,
    roots: &[PathBuf],
    workspace: &Path,
    target: &Path,
) -> Result<()> {
    check_path(workspace)?;
    check_path(target)?;
    if target != workspace.join("target") || !workspace.join("Cargo.toml").is_file() {
        return fail("Only verified workspace/target directories are supported");
    }
    let root =
        authorized_root(workspace, roots).ok_or("Workspace/target is outside authorized roots")?;
    if device(target)? != device(root)? {
        return fail("Target crosses an unauthorized mount boundary");
    }
    if overlaps(target, &config.cache_dir)? {
        return fail("Target overlaps managed cache");
    }
    for protected in [
        crate::config::cargo_home(),
        home().join(".rustup"),
        home().join(".rpi"),
    ] {
        if overlaps(target, &protected)? {
            return fail("Target overlaps tool configuration/storage");
        }
    }
    Ok(())
}

fn discover(
    config: &Config,
    roots: &[PathBuf],
    cargo: &Path,
    timeout: u64,
    report: &mut Report,
) -> Result<()> {
    let mut seen_dirs = BTreeSet::new();
    let mut seen_targets = BTreeSet::new();
    let mut pending: Vec<PathBuf> = roots.to_vec();
    while let Some(dir) = pending.pop() {
        if runner::interrupted() {
            return fail("Discovery interrupted");
        }
        if !seen_dirs.insert(dir.clone()) {
            continue;
        }
        let inspect = (|| -> Result<()> {
            check_path(&dir)?;
            if overlaps(&dir, &config.cache_dir)? && dir.starts_with(&config.cache_dir) {
                return Ok(());
            }
            let root = authorized_root(&dir, roots).ok_or("Directory outside scan roots")?;
            if device(&dir)? != device(root)? {
                return fail("Skipping nested mount");
            }
            if dir.join("Cargo.toml").is_file() {
                check_path(&dir.join("Cargo.toml"))?;
                // Avoid invoking Cargo (and expensive resolution) for projects with no default target.
                // Members can still resolve to a root with a target; that root is visited separately.
                if dir.join("target").try_exists()? {
                    check_path(&dir.join("target"))?;
                    let ws = workspace(cargo, &dir, timeout)?;
                    let target = ws.join("target");
                    validate_target(config, roots, &ws, &target)?;
                    if seen_targets.insert(target.clone()) {
                        let inventory = tree(&target)?;
                        let busy = (|| -> Result<()> {
                            let _lease = managed_lease(config, &ws)?;
                            let _locks = locks(&inventory.locks)?;
                            Ok(())
                        })()
                        .err()
                        .map(|e| e.to_string());
                        report.targets.push(TargetReport {
                            workspace: ws,
                            identity: identity(&target)?,
                            target,
                            before_bytes: inventory.bytes,
                            before: format_bytes(inventory.bytes),
                            removed_bytes: 0,
                            removed: "0 B".into(),
                            deletes_final_artifacts: true,
                            status: if busy.is_some() { "skipped" } else { "planned" }.into(),
                            reason: busy,
                        });
                    }
                }
            }
            Ok(())
        })();
        if let Err(error) = inspect {
            report.skipped.push(format!("{}: {error}", dir.display()));
        }
        // Even if this project cannot be resolved, nested independent workspaces may be valid.
        if check_path(&dir).is_err() {
            continue;
        }
        let Some(root) = authorized_root(&dir, roots) else {
            continue;
        };
        if device(&dir).ok() != device(root).ok() {
            continue;
        }
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) => {
                report.skipped.push(format!("{}: {e}", dir.display()));
                continue;
            }
        };
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(e) => {
                    report.skipped.push(e.to_string());
                    continue;
                }
            };
            let name = entry.file_name();
            if matches!(
                name.to_str(),
                Some(
                    ".git"
                        | ".rpi"
                        | ".cargo"
                        | ".rustup"
                        | "node_modules"
                        | "target"
                        | "vendor"
                        | ".deploy"
                )
            ) {
                continue;
            }
            let path = entry.path();
            if path.starts_with(&config.cache_dir) {
                continue;
            }
            match entry.file_type() {
                Ok(kind) if kind.is_symlink() => report
                    .skipped
                    .push(format!("{}: symlink not followed", path.display())),
                Ok(kind) if kind.is_dir() => pending.push(path),
                Ok(_) => {}
                Err(e) => report.skipped.push(format!("{}: {e}", path.display())),
            }
        }
    }
    report.targets.sort_by(|a, b| a.target.cmp(&b.target));
    Ok(())
}

fn observe_volume(path: &Path, report: &mut Report) -> Result<()> {
    let dev = device(path)?;
    if !report.volumes.iter().any(|v| v.device == dev) {
        let free = fs2::available_space(path)?;
        report.volumes.push(VolumeReport {
            path: path.into(),
            device: dev,
            free_before_bytes: free,
            free_after_bytes: free,
        });
    }
    Ok(())
}

fn managed_lease(config: &Config, workspace: &Path) -> Result<Option<Lease>> {
    if !config.cache_dir.is_dir() {
        return Ok(None);
    }
    crate::storage::check(config)?;
    let key = Store::key(workspace)?;
    existing_lock(&config.cache_dir.join("locks").join(format!("{key}.lock")))
}

fn clean_target(
    config: &Config,
    roots: &[PathBuf],
    cargo: &Path,
    target: &TargetReport,
    timeout: u64,
) -> Result<()> {
    validate_target(config, roots, &target.workspace, &target.target)?;
    if identity(&target.target)? != target.identity {
        return fail("Target directory was replaced after discovery");
    }
    // Hold the workspace lease (if present) through native clean, excluding managed builds.
    // When the external cache is offline, no replacement lock/root is created.
    let _lease = managed_lease(config, &target.workspace)?;
    let inventory = tree(&target.target)?;
    drop(locks(&inventory.locks)?); // Cargo must obtain its own locks; do not deadlock our child.
    check_version(cargo, &target.workspace, timeout)?;
    let mut command = command(cargo, &target.workspace, &target.target)?;
    command
        .args(["clean", "--offline", "--manifest-path"])
        .arg(target.workspace.join("Cargo.toml"))
        .arg("--target-dir")
        .arg(&target.target);
    run(&mut command, timeout)?;
    Ok(())
}

pub fn execute(config: &Config, options: &Options) -> Result<Report> {
    config.validate()?;
    if options.include_targets && !options.dry_run && !options.yes {
        return fail(
            "Deleting project target/final artifacts requires --yes; stop all producers and preview with --dry-run first",
        );
    }
    let timeout = if options.timeout_seconds == 0 {
        60
    } else {
        options.timeout_seconds
    };
    let mut report = Report {
        dry_run: options.dry_run,
        ..Report::default()
    };
    let mut roots = Vec::new();
    let mut real_cargo = None;
    if options.include_targets {
        let requested = if options.roots.is_empty() {
            &config.clean_roots
        } else {
            &options.roots
        };
        if requested.is_empty() {
            return fail("Global target cleanup requires --root or configured clean_roots");
        }
        for path in requested {
            let path = absolute(path)?;
            check_path(&path)?;
            let path = fs::canonicalize(path)?;
            if path.parent().is_none() || path == fs::canonicalize(home())? || !path.is_dir() {
                return fail("Refusing a filesystem root/home as a global cleanup root");
            }
            if overlaps(&path, &config.cache_dir)? && path.starts_with(&config.cache_dir) {
                return fail("Scan root is inside managed cache");
            }
            for protected in [
                crate::config::cargo_home(),
                home().join(".rustup"),
                home().join(".rpi"),
            ] {
                if overlaps(&path, &protected)? {
                    return fail("Scan root overlaps tool configuration/storage");
                }
            }
            if !roots.contains(&path) {
                roots.push(path);
            }
        }
        let cargo = cargo::resolve(&config.cargo, None)?;
        for root in &roots {
            observe_volume(root, &mut report)?;
        }
        discover(config, &roots, &cargo, timeout, &mut report)?;
        real_cargo = Some(cargo);
    }
    // Read-only opening avoids initializing a replacement cache or touching layout in dry-run.
    let managed = (|| -> Result<GcReport> {
        let store = Store::inspect(config.clone())?;
        observe_volume(&store.root, &mut report)?;
        store.collect(&GcOptions {
            all_idle: true,
            dry_run: options.dry_run,
            ..GcOptions::default()
        })
    })();
    match managed {
        Ok(gc) => {
            report.incomplete |= gc.busy || !gc.skipped.is_empty();
            report.managed_report = Some(gc);
        }
        Err(e) => {
            report.managed_error = Some(e.to_string());
            report.incomplete = true;
        }
    }
    if let Some(cargo) = real_cargo {
        for target in &mut report.targets {
            if runner::interrupted() {
                report.incomplete = true;
                break;
            }
            if target.status == "skipped" {
                report.incomplete = true;
                continue;
            }
            if options.dry_run {
                target.removed_bytes = target.before_bytes;
                target.removed = format_bytes(target.removed_bytes);
                continue;
            }
            let result = clean_target(config, &roots, &cargo, target, timeout);
            match result {
                Ok(()) => target.status = "cleaned".into(),
                Err(e) => {
                    target.status = "failed".into();
                    target.reason = Some(e.to_string());
                    report.incomplete = true;
                }
            }
            // A partial cleanup can free bytes even when native Cargo reports an error.
            match tree(&target.target) {
                Ok(after) => target.removed_bytes = target.before_bytes.saturating_sub(after.bytes),
                Err(e) => {
                    report.incomplete = true;
                    target.reason.get_or_insert_with(|| e.to_string());
                }
            }
            target.removed = format_bytes(target.removed_bytes);
        }
    }
    report.incomplete |= !report.skipped.is_empty();
    for volume in &mut report.volumes {
        if let Ok(free) = fs2::available_space(&volume.path) {
            volume.free_after_bytes = free;
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subprocess_timeout_does_not_wait_for_a_busy_producer_forever() {
        #[cfg(unix)]
        let mut command = {
            let mut c = Command::new("sleep");
            c.arg("30");
            c
        };
        #[cfg(windows)]
        let mut command = {
            let mut c = Command::new("powershell.exe");
            c.args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "Start-Sleep -Seconds 30",
            ]);
            c
        };
        let start = Instant::now();
        assert!(
            run(&mut command, 1)
                .unwrap_err()
                .to_string()
                .contains("timed out")
        );
        assert!(start.elapsed() < Duration::from_secs(15));
    }

    #[test]
    fn nonzero_subprocess_exit_is_an_error_not_successful_cleanup() {
        #[cfg(unix)]
        let mut command = {
            let mut c = Command::new("/bin/sh");
            c.args(["-c", "echo deliberate-failure >&2; exit 7"]);
            c
        };
        #[cfg(windows)]
        let mut command = {
            let mut c = Command::new("cmd");
            c.args(["/c", "echo deliberate-failure 1>&2 & exit /b 7"]);
            c
        };
        let error = run(&mut command, 10).unwrap_err().to_string();
        assert!(error.contains("deliberate-failure"), "{error}");
    }
}
