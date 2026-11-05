use crate::{
    Result, cargo,
    config::Config,
    fail,
    process::OwnedChild,
    store::{GcOptions, GcReport, Store},
    util::{GIB, absolute, atomic_write, check_path, now},
};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    ffi::{OsStr, OsString},
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    process::ExitStatus,
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant},
};

static INTERRUPTED: AtomicBool = AtomicBool::new(false);

pub fn install_signal_handler() -> Result<()> {
    ctrlc::set_handler(|| INTERRUPTED.store(true, Ordering::SeqCst))?;
    Ok(())
}

fn toolchain(args: &[OsString]) -> (&[OsString], &[OsString]) {
    if args
        .first()
        .is_some_and(|s| s.to_string_lossy().starts_with('+'))
    {
        args.split_at(1)
    } else {
        (&[], args)
    }
}

/// Find the Cargo command without altering the original OS argument vector.
fn command_index(args: &[OsString]) -> Option<usize> {
    let mut index = 0;
    while index < args.len() {
        let text = args[index].to_string_lossy();
        if matches!(text.as_ref(), "--config" | "--color" | "-Z" | "-C") {
            index += 2;
            continue;
        }
        if !text.starts_with('-') {
            return Some(index);
        }
        index += 1;
    }
    None
}

fn install_read_only(args: &[OsString]) -> bool {
    // Native Cargo decides whether malformed combinations are valid. These
    // exact flags cannot be option values unless attached with '='; '--' ends
    // option parsing and prevents a package name from becoming a help request.
    before_separator(args)
        .iter()
        .any(|arg| arg == "--list" || arg == "--help" || arg == "-h")
}

/// Digest the complete invocation, canonicalizing local source paths. No raw
/// arguments, URL credentials, registry tokens or source paths leave this function.
fn install_identity(
    cargo: &Path,
    version: &str,
    tc: &[OsString],
    args: &[OsString],
) -> Result<String> {
    let mut hash = Sha256::new();
    let mut add = |value: &OsStr| {
        let bytes = value.as_encoded_bytes();
        hash.update((bytes.len() as u64).to_le_bytes());
        hash.update(bytes);
    };
    add(OsStr::new("cargoc-install-v1"));
    add(cargo.as_os_str());
    add(OsStr::new(version));
    for arg in tc {
        add(arg);
    }
    add(&std::env::var_os("RUSTUP_TOOLCHAIN").unwrap_or_default());
    // The path source itself is canonicalized, so --path aliases share a cache.
    // Other flags are deliberately included: different registry/revision/feature
    // selections are isolated, while identical invocations share a lease.
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if arg == "--" {
            for value in &args[index..] {
                add(value);
            }
            break;
        }
        let text = arg.to_string_lossy();
        if arg == "--path" || text.starts_with("--path=") {
            let path = if arg == "--path" {
                index += 1;
                args.get(index).ok_or("Missing Cargo --path value")?.clone()
            } else {
                option_tail(arg, "--path=")
            };
            let canonical = fs::canonicalize(Path::new(&path))
                .map_err(|_| "Cannot resolve Cargo install --path source")?;
            add(OsStr::new("--path"));
            #[cfg(windows)]
            add(OsStr::new(
                &crate::util::identity_path(&canonical)
                    .map_err(|_| "Cannot identify Cargo install --path source")?,
            ));
            #[cfg(not(windows))]
            add(canonical.as_os_str());
        } else {
            add(arg);
        }
        index += 1;
    }
    Ok(format!("{:x}", hash.finalize()))
}

/// Keep install sources/final outputs out of the disposable ownership tree.
fn validate_install_paths(config: &Config, args: &[OsString]) -> Result<()> {
    let check = |path: &OsStr| -> Result<()> {
        if crate::util::same_or_descendant(Path::new(path), &config.cache_dir)
            .map_err(|_| "Cannot identify Cargo install source/output path")?
        {
            return fail(
                "Cargo install sources and final outputs must be outside the managed cache root",
            );
        }
        Ok(())
    };
    for name in ["CARGO_INSTALL_ROOT", "CARGO_HOME", "CARGO_TARGET_DIR"] {
        if let Some(value) = std::env::var_os(name).filter(|v| !v.is_empty()) {
            check(&value)?;
        }
    }
    let mut args = before_separator(args).iter();
    while let Some(arg) = args.next() {
        for flag in ["--path", "--root", "--target-dir"] {
            if arg == flag {
                if let Some(value) = args.next() {
                    check(value)?;
                }
                break;
            }
            let prefix = format!("{flag}=");
            if arg.to_string_lossy().starts_with(&prefix) {
                check(&option_tail(arg, &prefix))?;
                break;
            }
        }
    }
    Ok(())
}

/// Validate explicit Cargo config sources that can redirect install outputs.
/// Cargo's unobserved global config is intentionally not treated as validated.
fn validate_install_config_paths(config: &Config, args: &[OsString]) -> Result<()> {
    let check = |name: &str, value: &toml::Value, base: &Path| -> Result<()> {
        let path = value
            .as_str()
            .ok_or_else(|| format!("Cargo --config {name} must be a path string"))?;
        if crate::util::same_or_descendant(&base.join(path), &config.cache_dir)
            .map_err(|_| "Cannot identify Cargo install source/output path")?
        {
            return fail(format!(
                "Cargo --config {name} points inside the managed cache root"
            ));
        }
        Ok(())
    };
    let validate = |content: &str, base: &Path| -> Result<()> {
        let data: toml::Value =
            toml::from_str(content).map_err(|_| "Invalid Cargo --config value")?;
        if let Some(value) = data.get("install").and_then(|section| section.get("root")) {
            check("install.root", value, base)?;
        }
        if let Some(value) = data
            .get("build")
            .and_then(|section| section.get("target-dir"))
        {
            check("build.target-dir", value, base)?;
        }
        Ok(())
    };
    let before = before_separator(args);
    let mut index = 0;
    while index < before.len() {
        let text = before[index].to_string_lossy();
        let value = if text == "--config" {
            index += 1;
            before.get(index).ok_or("Missing Cargo --config value")?
        } else if text.starts_with("--config=") {
            &before[index]
        } else {
            index += 1;
            continue;
        };
        let value = if text == "--config" {
            value.to_os_string()
        } else {
            option_tail(value, "--config=")
        };
        let (content, base) = if value.to_string_lossy().contains('=') {
            (
                value.to_string_lossy().into_owned(),
                std::env::current_dir()?,
            )
        } else {
            let file = absolute(Path::new(&value))?;
            // Cargo resolves paths from config files against the parent of
            // the containing directory (also for extra --config files).
            let base = file
                .parent()
                .and_then(Path::parent)
                .unwrap_or(Path::new("/"));
            (fs::read_to_string(&file)?, base.to_path_buf())
        };
        validate(&content, &base)?;
        index += 1;
    }
    Ok(())
}

fn option_tail(value: &OsStr, prefix: &str) -> OsString {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::{OsStrExt, OsStringExt};
        OsString::from_wide(&value.encode_wide().skip(prefix.len()).collect::<Vec<_>>())
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::ffi::{OsStrExt, OsStringExt};
        OsString::from_vec(value.as_bytes()[prefix.len()..].to_vec())
    }
}

fn before_separator(args: &[OsString]) -> &[OsString] {
    let index = args.iter().position(|s| s == "--").unwrap_or(args.len());
    &args[..index]
}

pub fn validate_cargo_args(args: &[OsString]) -> Result<()> {
    let before = before_separator(args);
    for (i, arg) in before.iter().enumerate() {
        let text = arg.to_string_lossy();
        if text == "--build-dir" || text.starts_with("--build-dir=") {
            return fail("Use cargoc config --cache-dir, not Cargo --build-dir");
        }
        if text == "-C" || text.starts_with("-C=") {
            return fail("Cargo -C is unsupported; change directory or use --manifest-path");
        }
        let value = if text == "--config" {
            Some(
                before
                    .get(i + 1)
                    .ok_or("Missing Cargo --config value")?
                    .to_string_lossy()
                    .into_owned(),
            )
        } else {
            text.strip_prefix("--config=").map(str::to_string)
        };
        if let Some(value) = value {
            let content = if value.contains('=') {
                value.clone()
            } else {
                fs::read_to_string(&value)?
            };
            let data: toml::Value =
                toml::from_str(&content).map_err(|_| "Invalid Cargo --config value")?;
            if data.get("build").and_then(|v| v.get("build-dir")).is_some() {
                return fail("Cargo --config cannot override build.build-dir in cargoc");
            }
        }
    }
    Ok(())
}

pub fn locate_workspace(cargo: &Path, args: &[OsString]) -> Result<PathBuf> {
    let (tc, rest) = toolchain(args);
    let mut command = crate::cargo::command(cargo)?;
    command
        .args(tc)
        .args(["locate-project", "--workspace", "--message-format", "plain"]);
    let before = before_separator(rest);
    let mut index = 0;
    while index < before.len() {
        let text = before[index].to_string_lossy();
        if text == "--manifest-path" || text == "--config" {
            command
                .arg(&before[index])
                .arg(before.get(index + 1).ok_or("Missing Cargo option value")?);
            index += 1;
        } else if text.starts_with("--manifest-path=") || text.starts_with("--config=") {
            command.arg(&before[index]);
        }
        index += 1;
    }
    let output = command.output()?;
    if !output.status.success() {
        return fail(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    let manifest = absolute(Path::new(String::from_utf8(output.stdout)?.trim()))?;
    if !manifest.is_file() {
        return fail("Cargo returned an invalid workspace manifest");
    }
    Ok(manifest
        .parent()
        .ok_or("Workspace has no parent")?
        .to_path_buf())
}

fn check_version(cargo: &Path, tc: &[OsString]) -> Result<String> {
    let output = crate::cargo::command(cargo)?
        .args(tc)
        .arg("--version")
        .output()?;
    let text = String::from_utf8_lossy(&output.stdout);
    if !output.status.success() || crate::cargo::version_number(&text).is_none_or(|v| v < (1, 91)) {
        return fail("Managed build-dir requires Cargo >= 1.91");
    }
    Ok(text.into_owned())
}

pub fn exit_code(status: ExitStatus) -> i32 {
    if let Some(code) = status.code() {
        return code;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        128 + status.signal().unwrap_or(1)
    }
    #[cfg(not(unix))]
    {
        1
    }
}

fn event(config_path: &Path, value: &serde_json::Value) {
    let write = || -> Result<()> {
        let path = config_path
            .parent()
            .ok_or("Config has no parent")?
            .join("events.jsonl");
        check_path(&path)?;
        if path.exists() && path.metadata()?.len() > 2 * 1024 * 1024 {
            // Rotation uses atomic replacement as well, avoiding symlink following.
            let backup = path.with_extension("previous.jsonl");
            atomic_write(&backup, &fs::read(&path)?)?;
            atomic_write(&path, b"")?;
        }
        let mut file = OpenOptions::new().create(true).append(true).open(path)?;
        writeln!(file, "{}", json!({"time": now(), "event": value}))?;
        Ok(())
    };
    if let Err(e) = write() {
        eprintln!("cargoc: warning: cannot write log: {e}");
    }
}

fn gc_report(config_path: &Path, report: &GcReport) {
    if report.removed_bytes > 0 {
        eprintln!(
            "cargoc: cleaned {:.2} GiB of idle intermediate files",
            report.removed_bytes as f64 / GIB
        );
        event(config_path, &json!({"kind": "clean", "report": report}));
    }
    if !report.skipped.is_empty() {
        eprintln!(
            "cargoc: protected/skipped {} cache(s)",
            report.skipped.len()
        );
    }
}

fn automatic_gc(store: &Store, config_path: &Path, options: &GcOptions) -> Option<GcReport> {
    match store.collect(options) {
        Ok(report) => {
            gc_report(config_path, &report);
            Some(report)
        }
        Err(error) => {
            eprintln!("cargoc: warning: safe cleanup unavailable: {error}");
            None
        }
    }
}

fn version_request(args: &[OsString]) -> bool {
    let mut args = before_separator(args).iter();
    while let Some(arg) = args.next() {
        let text = arg.to_string_lossy();
        if text == "--version"
            || text == "-V"
            || text == "version"
            || (text.starts_with('-')
                && !text.starts_with("--")
                && text.contains('V')
                && text[1..].chars().all(|c| matches!(c, 'v' | 'V' | 'q')))
        {
            return true;
        }
        if matches!(text.as_ref(), "--color" | "--config" | "-Z" | "-C") {
            args.next();
        } else if !text.starts_with('-') {
            return false;
        }
    }
    false
}

pub fn run(config: Config, config_path: &Path, args: &[OsString]) -> Result<i32> {
    run_with_free(config, config_path, args, |store| store.free())
}

fn run_with_free<F>(
    mut config: Config,
    config_path: &Path,
    args: &[OsString],
    free: F,
) -> Result<i32>
where
    F: Fn(&Store) -> Result<u64>,
{
    if args.is_empty() {
        return fail("Missing Cargo arguments");
    }
    config.cargo = cargo::resolve(&config.cargo, None)?;
    let (tc, rest) = toolchain(args);
    if version_request(rest) {
        let status = cargo::command(&config.cargo)?.args(args).status()?;
        if status.success() {
            println!("cargoc {}", env!("CARGO_PKG_VERSION"));
        }
        return Ok(exit_code(status));
    }
    let command_at = command_index(rest);
    let command_name = command_at.and_then(|i| rest[i].to_str());
    let installing = command_name == Some("install");
    let passthrough = [
        "--help",
        "-h",
        "version",
        "help",
        "fmt",
        "new",
        "init",
        "clean",
        "uninstall",
        "search",
        "info",
        "login",
        "logout",
        "publish",
        "owner",
        "yank",
        "fetch",
        "update",
        "metadata",
        "locate-project",
    ];
    if (installing && install_read_only(&rest[command_at.unwrap() + 1..]))
        || command_name.is_some_and(|s| passthrough.contains(&s))
        || rest[..command_at.unwrap_or(rest.len())]
            .iter()
            .any(|s| s == "--help" || s == "-h")
    {
        eprintln!("cargoc: passthrough (this command does not use the managed cache)");
        return Ok(exit_code(
            cargo::command(&config.cargo)?.args(args).status()?,
        ));
    }
    validate_cargo_args(args)?;
    let cargo_version = check_version(&config.cargo, tc)?;
    let identity = if installing {
        validate_install_paths(&config, rest)?;
        validate_install_config_paths(&config, rest)?;
        Some(install_identity(&config.cargo, &cargo_version, tc, rest)?)
    } else {
        None
    };
    let workspace = if installing {
        None
    } else {
        Some(locate_workspace(&config.cargo, args)?)
    };
    let store = Store::open(config.clone(), true)?;
    let (key, build_dir) = if let Some(identity) = &identity {
        store.install(identity)?
    } else {
        store.workspace(workspace.as_deref().ok_or("Missing workspace")?)?
    };
    let touch = || {
        if let Some(identity) = &identity {
            store.touch_install(identity)
        } else {
            store.touch(workspace.as_deref().ok_or("Missing workspace")?)
        }
    };
    // Install events use only an opaque digest, never raw arguments or URLs.
    let subject = if installing {
        json!({"install": key})
    } else {
        json!({"workspace": workspace})
    };
    if config.auto_clean {
        automatic_gc(&store, config_path, &GcOptions::default());
    }
    let result;
    {
        let _lease = store
            .lease(&key)?
            .ok_or("Cache is busy; another cargoc invocation is using it")?;
        touch()?;
        if (free(&store)? as f64) < config.min_free_gib.max(config.abort_free_gib) * GIB {
            return fail("Insufficient free disk after safe cleanup; refusing to start Cargo");
        }
        let path_text = build_dir
            .to_str()
            .ok_or("Cache path must be valid UTF-8 for Cargo config")?
            .to_owned();
        let build_config = format!("build.build-dir={}", toml::Value::String(path_text));
        let mut command = cargo::command(&config.cargo)?;
        command
            .args(tc)
            .arg("--config")
            .arg(build_config)
            .args(rest)
            .env("CARGO_BUILD_BUILD_DIR", &build_dir);
        if let Some(workspace) = &workspace {
            eprintln!("cargoc: workspace: {}", workspace.display());
        } else {
            eprintln!("cargoc: managed install cache: {key}");
        }
        eprintln!("cargoc: intermediate cache: {}", build_dir.display());
        let mut child = OwnedChild::spawn(&mut command)?;
        let mut next_check = Instant::now() + Duration::from_secs_f64(config.poll_seconds);
        let mut next_warning = Instant::now();
        result = loop {
            if INTERRUPTED.load(Ordering::SeqCst) {
                child.stop()?;
                break 130;
            }
            if let Some(status) = child.status()? {
                break exit_code(status);
            }
            if config.abort_free_gib > 0.0 && (free(&store)? as f64) < config.abort_free_gib * GIB {
                eprintln!(
                    "cargoc: emergency disk threshold; stopping ONLY this Cargo process tree"
                );
                let mut stop_event = subject.clone();
                stop_event["kind"] = json!("emergency-stop");
                event(config_path, &stop_event);
                child.stop()?;
                break 2;
            }
            if Instant::now() >= next_check {
                if config.auto_clean {
                    let report = automatic_gc(
                        &store,
                        config_path,
                        &GcOptions {
                            protected: [key.clone()].into(),
                            ..GcOptions::default()
                        },
                    );
                    if report.is_some_and(|report| report.unresolved)
                        && Instant::now() >= next_warning
                    {
                        eprintln!(
                            "cargoc: space target cannot currently be met; active caches remain protected"
                        );
                        next_warning = Instant::now() + Duration::from_secs(30);
                    }
                }
                next_check = Instant::now() + Duration::from_secs_f64(config.poll_seconds);
            }
            thread::sleep(Duration::from_secs_f64(config.poll_seconds.min(0.25)));
        };
        drop(child); // Close the process group/job before releasing its cache lease.
        if let Err(error) = touch() {
            eprintln!("cargoc: warning: cannot update last-used record: {error}");
        }
    }
    // Now even this workspace may be cleaned if it alone exceeds the size budget.
    if config.auto_clean {
        automatic_gc(&store, config_path, &GcOptions::default());
    }
    let mut exit_event = subject;
    exit_event["kind"] = json!("cargo-exit");
    exit_event["exit_code"] = json!(result);
    event(config_path, &exit_event);
    Ok(result)
}

pub fn os_args(values: &[&str]) -> Vec<OsString> {
    values
        .iter()
        .map(OsStr::new)
        .map(OsStr::to_os_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        process::Command,
        sync::atomic::{AtomicUsize, Ordering},
        thread,
        time::Duration,
    };
    use tempfile::TempDir;

    fn helper(temp: &TempDir) -> PathBuf {
        let source = temp.path().join("fake-cargo.rs");
        let executable = temp
            .path()
            .join(format!("fake-cargo{}", std::env::consts::EXE_SUFFIX));
        let marker = serde_json::to_string(
            &temp
                .path()
                .join("version-hook")
                .to_string_lossy()
                .into_owned(),
        )
        .unwrap();
        let code = format!(
            r#"
use std::{{env, fs, process::{{self, Command}}, thread, time::Duration}};
const VERSION_MARKER: &str = {marker};
fn value(args: &[String], name: &str) -> String {{
    args.windows(2).find(|pair| pair[0] == name).map(|pair| pair[1].clone()).unwrap()
}}
fn main() {{
    let args: Vec<String> = env::args().skip(1).collect();
    if args.first().is_some_and(|arg| arg == "--runner-child") {{
        fs::write(&args[1], process::id().to_string()).unwrap();
        loop {{ thread::sleep(Duration::from_secs(30)); }}
    }}
    if args.iter().any(|arg| arg == "--version") {{
        fs::write(VERSION_MARKER, "checked").unwrap();
        println!("cargo 1.99.0 (runner-test)");
        return;
    }}
    let started = value(&args, "--started-file");
    fs::write(&started, "started").unwrap();
    if args.iter().any(|arg| arg == "--oneshot") {{
        thread::sleep(Duration::from_millis(150));
        return;
    }}
    let child_file = value(&args, "--child-file");
    let child = Command::new(env::current_exe().unwrap())
        .args(["--runner-child", &child_file])
        .spawn()
        .unwrap();
    let _ = child;
    loop {{ thread::sleep(Duration::from_secs(30)); }}
}}
"#,
            marker = marker
        );
        fs::write(&source, code).unwrap();
        let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
        let output = Command::new(rustc)
            .args(["--edition=2021"])
            .arg(&source)
            .arg("-o")
            .arg(&executable)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        executable
    }

    fn config(temp: &TempDir, cargo: PathBuf) -> (Config, PathBuf) {
        let config_path = temp.path().join("cargoc.toml");
        let config = Config {
            cache_dir: temp.path().join("cache"),
            cargo,
            auto_clean: false,
            min_free_gib: 0.0,
            abort_free_gib: 1.0,
            poll_seconds: 0.1,
            ..Config::default()
        };
        config.save(&config_path).unwrap();
        (config, config_path)
    }

    fn alive(pid: u32) -> bool {
        #[cfg(unix)]
        {
            // SAFETY: This only probes the test helper PID; it sends no signal.
            #[allow(unsafe_code)]
            unsafe {
                if libc::kill(pid as i32, 0) != 0 {
                    return false;
                }
            }
            // A killed orphan may remain a zombie until init reaps it.
            Command::new("ps")
                .args(["-o", "stat=", "-p", &pid.to_string()])
                .output()
                .is_ok_and(|output| {
                    let state = String::from_utf8_lossy(&output.stdout);
                    !state.trim().is_empty() && !state.trim_start().starts_with('Z')
                })
        }
        #[cfg(windows)]
        {
            let filter = format!("PID eq {pid}");
            Command::new("tasklist")
                .args(["/FI", &filter, "/NH"])
                .output()
                .is_ok_and(|output| {
                    String::from_utf8_lossy(&output.stdout).contains(&pid.to_string())
                })
        }
    }

    fn assert_stopped(pid: u32) {
        for _ in 0..40 {
            if !alive(pid) {
                return;
            }
            thread::sleep(Duration::from_millis(50));
        }
        panic!("owned helper child {pid} survived stop");
    }

    #[test]
    fn emergency_stop_uses_injected_probe_and_releases_install_lease() {
        let temp = tempfile::tempdir().unwrap();
        let cargo = helper(&temp);
        let (config, config_path) = config(&temp, cargo);
        let started = temp.path().join("started");
        let child_file = temp.path().join("child-pid");
        let args = [
            OsString::from("install"),
            OsString::from("demo"),
            OsString::from("--started-file"),
            started.clone().into_os_string(),
            OsString::from("--child-file"),
            child_file.clone().into_os_string(),
        ];
        let calls = AtomicUsize::new(0);
        let result = run_with_free(config.clone(), &config_path, &args, |_| {
            if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                Ok(u64::MAX)
            } else {
                let deadline = Instant::now() + Duration::from_secs(10);
                while fs::read_to_string(&child_file)
                    .ok()
                    .and_then(|text| text.parse::<u32>().ok())
                    .is_none()
                {
                    assert!(Instant::now() < deadline, "helper child did not start");
                    thread::sleep(Duration::from_millis(5));
                }
                Ok(0)
            }
        })
        .unwrap();
        assert_eq!(result, 2);
        assert!(started.is_file());
        let pid: u32 = fs::read_to_string(child_file).unwrap().parse().unwrap();
        assert_stopped(pid);
        let store = Store::open(config, false).unwrap();
        let entry = store.entries().unwrap().pop().unwrap();
        assert!(store.lease(&entry.record.key).unwrap().is_some());
        let events = fs::read_to_string(temp.path().join("events.jsonl")).unwrap();
        assert!(events.contains("emergency-stop"));
        assert!(events.contains(&entry.record.key));
        assert!(!events.contains("demo"));
    }

    #[test]
    fn probe_error_stops_owned_tree_before_releasing_lease() {
        let temp = tempfile::tempdir().unwrap();
        let cargo = helper(&temp);
        let (config, config_path) = config(&temp, cargo);
        let started = temp.path().join("started");
        let child_file = temp.path().join("child-pid");
        let args = [
            OsString::from("install"),
            OsString::from("probe-error"),
            OsString::from("--started-file"),
            started.clone().into_os_string(),
            OsString::from("--child-file"),
            child_file.clone().into_os_string(),
        ];
        let calls = AtomicUsize::new(0);
        let result = run_with_free(config.clone(), &config_path, &args, |_| {
            if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                Ok(u64::MAX)
            } else {
                let deadline = Instant::now() + Duration::from_secs(10);
                while fs::read_to_string(&child_file)
                    .ok()
                    .and_then(|text| text.parse::<u32>().ok())
                    .is_none()
                {
                    assert!(Instant::now() < deadline, "helper child did not start");
                    thread::sleep(Duration::from_millis(5));
                }
                Err(std::io::Error::other("probe failed").into())
            }
        });
        assert!(result.is_err());
        let pid: u32 = fs::read_to_string(child_file).unwrap().parse().unwrap();
        assert_stopped(pid);
        let store = Store::open(config, false).unwrap();
        let entry = store.entries().unwrap().pop().unwrap();
        assert!(store.lease(&entry.record.key).unwrap().is_some());
    }

    #[test]
    fn spawn_failure_releases_install_lease_after_version_hook() {
        let temp = tempfile::tempdir().unwrap();
        let cargo = helper(&temp);
        let (config, config_path) = config(&temp, cargo.clone());
        let args = os_args(&["install", "spawn-failure", "--oneshot"]);
        // The pre-start probe runs after the version command has exited but
        // before spawning the build, so no scheduling race is necessary.
        let result = run_with_free(config.clone(), &config_path, &args, |_| {
            assert!(temp.path().join("version-hook").is_file());
            fs::remove_file(&cargo)?;
            Ok(u64::MAX)
        });
        assert!(result.is_err());
        let store = Store::open(config, false).unwrap();
        let entry = store.entries().unwrap().pop().unwrap();
        assert!(store.lease(&entry.record.key).unwrap().is_some());
    }

    #[test]
    fn interruption_stops_install_tree_and_releases_lease() {
        const CHILD: &str = "CARGOC_RUNNER_INTERRUPT_TEST";
        if std::env::var_os(CHILD).is_none() {
            // The signal flag is process-global; isolate this test from every
            // other concurrent runner instead of resetting it in a shared test.
            let output = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "runner::tests::interruption_stops_install_tree_and_releases_lease",
                    "--nocapture",
                ])
                .env(CHILD, "1")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "stdout={} stderr={}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let cargo = helper(&temp);
        let (config, config_path) = config(&temp, cargo);
        let child_file = temp.path().join("child-pid");
        let args = [
            OsString::from("install"),
            OsString::from("interrupt"),
            OsString::from("--started-file"),
            temp.path().join("started").into_os_string(),
            OsString::from("--child-file"),
            child_file.clone().into_os_string(),
        ];
        let calls = AtomicUsize::new(0);
        let code = run_with_free(config.clone(), &config_path, &args, |_| {
            if calls.fetch_add(1, Ordering::SeqCst) > 0 {
                let deadline = Instant::now() + Duration::from_secs(10);
                while fs::read_to_string(&child_file)
                    .ok()
                    .and_then(|text| text.parse::<u32>().ok())
                    .is_none()
                {
                    assert!(Instant::now() < deadline, "helper child did not start");
                    thread::sleep(Duration::from_millis(5));
                }
                INTERRUPTED.store(true, Ordering::SeqCst);
            }
            Ok(u64::MAX)
        })
        .unwrap();
        assert_eq!(code, 130);
        assert_stopped(fs::read_to_string(child_file).unwrap().parse().unwrap());
        let store = Store::open(config, false).unwrap();
        let entry = store.entries().unwrap().pop().unwrap();
        assert!(store.lease(&entry.record.key).unwrap().is_some());
        assert!(
            fs::read_to_string(temp.path().join("events.jsonl"))
                .unwrap()
                .contains("\"exit_code\":130")
        );
    }

    #[test]
    fn zero_abort_threshold_does_not_emergency_stop_when_probe_is_zero() {
        let temp = tempfile::tempdir().unwrap();
        let cargo = helper(&temp);
        let (mut config, config_path) = config(&temp, cargo);
        config.abort_free_gib = 0.0;
        let started = temp.path().join("started");
        let args = [
            OsString::from("install"),
            OsString::from("--oneshot"),
            OsString::from("--started-file"),
            started.into_os_string(),
            OsString::from("--child-file"),
            temp.path().join("unused-child").into_os_string(),
        ];
        let result = run_with_free(config, &config_path, &args, |_| Ok(0)).unwrap();
        assert_eq!(result, 0);
    }
}
