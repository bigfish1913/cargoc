use crate::{
    Result, cargo,
    config::Config,
    fail,
    process::OwnedChild,
    store::{GcOptions, GcReport, Store},
    util::{GIB, absolute, atomic_write, check_path, now},
};
use serde_json::json;
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
            let data: toml::Value = toml::from_str(&content)?;
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

fn check_version(cargo: &Path, tc: &[OsString]) -> Result<()> {
    let output = crate::cargo::command(cargo)?
        .args(tc)
        .arg("--version")
        .output()?;
    let text = String::from_utf8_lossy(&output.stdout);
    if !output.status.success() || crate::cargo::version_number(&text).is_none_or(|v| v < (1, 91)) {
        return fail("Managed build-dir requires Cargo >= 1.91");
    }
    Ok(())
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

pub fn run(mut config: Config, config_path: &Path, args: &[OsString]) -> Result<i32> {
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
    let passthrough = [
        "--help",
        "-h",
        "version",
        "help",
        "fmt",
        "new",
        "init",
        "install",
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
    if rest
        .first()
        .and_then(|s| s.to_str())
        .is_some_and(|s| passthrough.contains(&s))
    {
        eprintln!("cargoc: passthrough (this command does not use the managed cache)");
        return Ok(exit_code(
            cargo::command(&config.cargo)?.args(args).status()?,
        ));
    }
    validate_cargo_args(args)?;
    check_version(&config.cargo, tc)?;
    let workspace = locate_workspace(&config.cargo, args)?;
    let store = Store::open(config.clone(), true)?;
    let (key, build_dir) = store.workspace(&workspace)?;
    if config.auto_clean {
        automatic_gc(&store, config_path, &GcOptions::default());
    }
    let result;
    {
        let _lease = store
            .lease(&key)?
            .ok_or("Workspace is busy; another cargoc invocation is using it")?;
        store.touch(&workspace)?;
        if (store.free()? as f64) < config.min_free_gib.max(config.abort_free_gib) * GIB {
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
        eprintln!(
            "cargoc: workspace: {}\ncargoc: intermediate cache: {}",
            workspace.display(),
            build_dir.display()
        );
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
            if config.abort_free_gib > 0.0 && (store.free()? as f64) < config.abort_free_gib * GIB {
                eprintln!(
                    "cargoc: emergency disk threshold; stopping ONLY this Cargo process tree"
                );
                event(
                    config_path,
                    &json!({"kind": "emergency-stop", "workspace": workspace}),
                );
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
        if let Err(error) = store.touch(&workspace) {
            eprintln!("cargoc: warning: cannot update last-used record: {error}");
        }
    }
    // Now even this workspace may be cleaned if it alone exceeds the size budget.
    if config.auto_clean {
        automatic_gc(&store, config_path, &GcOptions::default());
    }
    event(
        config_path,
        &json!({"kind": "cargo-exit", "workspace": workspace, "exit_code": result}),
    );
    Ok(result)
}

pub fn os_args(values: &[&str]) -> Vec<OsString> {
    values
        .iter()
        .map(OsStr::new)
        .map(OsStr::to_os_string)
        .collect()
}
