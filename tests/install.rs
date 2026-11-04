//! Installs never contact a registry: all registry/git cases use a local Cargo
//! stand-in; only the dependency-free --offline --path smoke test uses real Cargo.
use cargoc::{
    config::Config,
    store::{CacheKind, GcOptions, Lease, Store},
    util::GIB,
};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    sync::OnceLock,
    thread,
    time::{Duration, Instant},
};
use tempfile::TempDir;

fn temp() -> TempDir {
    tempfile::tempdir_in(fs::canonicalize(std::env::temp_dir()).unwrap()).unwrap()
}

fn fake_cargo() -> &'static Path {
    static COMPILED: OnceLock<(TempDir, PathBuf)> = OnceLock::new();
    &COMPILED.get_or_init(|| {
        let dir = temp();
        let source = dir.path().join("fake.rs");
        fs::write(&source, r#"
use std::{env, fs::{self, OpenOptions}, io::Write, path::PathBuf, process, thread, time::Duration};
fn main() {
    let args: Vec<_> = env::args_os().skip(1).collect();
    let mut trace = OpenOptions::new().create(true).append(true).open(env::var_os("INSTALL_TRACE").unwrap()).unwrap();
    writeln!(trace, "{args:?}").unwrap();
    if args.iter().any(|a| a == "locate-project") { eprintln!("INSTALL MUST NOT LOCATE"); process::exit(99); }
    if args.iter().any(|a| a == "--version") && !args.iter().any(|a| a == "install") {
        println!("cargo {} (fixture)", env::var("INSTALL_CARGO_VERSION").unwrap_or("1.99.0".into())); return;
    }
    if args.iter().any(|a| a == "--list" || a == "--help" || a == "-h" || a == "help") {
        assert!(env::var_os("CARGO_BUILD_BUILD_DIR").is_none());
        assert!(!args.iter().any(|a| a.to_string_lossy().contains("build.build-dir")));
        println!("native install information"); return;
    }
    if let Some(path) = env::var_os("INSTALL_EXPECT_REMOVED") { assert!(!PathBuf::from(path).exists(), "pre-GC did not remove idle cache"); }
    let dir = PathBuf::from(env::var_os("CARGO_BUILD_BUILD_DIR").expect("managed build-dir"));
    assert!(args.iter().any(|a| a.to_string_lossy().contains("build.build-dir")));
    fs::write(env::var_os("INSTALL_ARGS").unwrap(), format!("{args:?}")).unwrap();
    fs::create_dir_all(dir.join("release/deps")).unwrap();
    fs::write(dir.join("release/.cargo-lock"), "").unwrap();
    let cached = dir.join("release/deps/cache.bin");
    fs::write(&cached, vec![42; 8192]).unwrap();
    let root = PathBuf::from(env::var_os("INSTALL_ROOT").unwrap());
    fs::create_dir_all(root.join("bin")).unwrap();
    fs::write(root.join("bin/installed"), "final installed binary").unwrap();
    fs::write(env::var_os("INSTALL_STARTED").unwrap(), "started").unwrap();
    match env::var("INSTALL_MODE").unwrap_or_default().as_str() {
        "fail" => process::exit(7),
        "post-error" => {
            fs::write(dir.parent().unwrap().parent().unwrap().join("state/invalid.json"), "{}").unwrap();
            process::exit(7);
        },
        "wait" => { thread::sleep(Duration::from_millis(1200)); if !cached.is_file() { process::exit(55); } },
        _ => {},
    }
}
"#).unwrap();
        let executable = dir.path().join(format!("fake-cargo{}", std::env::consts::EXE_SUFFIX));
        let output = Command::new(std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()))
            .args(["--edition=2021"]).arg(source).arg("-o").arg(&executable).output().unwrap();
        success(&output);
        (dir, executable)
    }).1
}

struct Fixture {
    temp: TempDir,
    config: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let temp = temp();
        // Deliberately not a Cargo workspace, and no manifest anywhere above it.
        fs::create_dir_all(temp.path().join("outside")).unwrap();
        fs::create_dir_all(temp.path().join("cargo-home")).unwrap();
        fs::create_dir_all(temp.path().join("home")).unwrap();
        let config = temp.path().join("cargoc.toml");
        Config {
            cargo: fake_cargo().to_owned(),
            cache_dir: temp.path().join("cache"),
            min_free_gib: 0.0,
            abort_free_gib: 0.0,
            max_age_days: 0.0,
            poll_seconds: 0.1,
            ..Config::default()
        }
        .save(&config)
        .unwrap();
        Self { temp, config }
    }
    fn path(&self, path: &str) -> PathBuf {
        self.temp.path().join(path)
    }
    fn cfg(&self) -> Config {
        Config::load(&self.config).unwrap()
    }
    fn configure(&self, update: impl FnOnce(&mut Config)) {
        let mut cfg = self.cfg();
        update(&mut cfg);
        cfg.save(&self.config).unwrap();
    }
    fn command(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_cargoc"));
        cmd.arg("--cargoc-config")
            .arg(&self.config)
            .current_dir(self.path("outside"))
            .env("CARGOC_DISABLE_UPDATE_CHECK", "1")
            .env("CARGO_NET_OFFLINE", "true")
            .env("HOME", self.path("home"))
            .env("USERPROFILE", self.path("home"))
            .env("CARGO_HOME", self.path("cargo-home"))
            .env_remove("CARGO_BUILD_BUILD_DIR")
            .env_remove("CARGO_TARGET_DIR")
            .env("INSTALL_TRACE", self.path("trace"))
            .env("INSTALL_ARGS", self.path("args"))
            .env("INSTALL_ROOT", self.path("installed"))
            .env("INSTALL_STARTED", self.path("started"));
        cmd
    }
    fn run(&self, args: &[&str]) -> Output {
        self.command().arg("exec").args(args).output().unwrap()
    }
    fn store(&self) -> Store {
        Store::open(self.cfg(), false).unwrap()
    }
    fn key(&self) -> String {
        self.store().entries().unwrap()[0].record.key.clone()
    }
    fn cached(&self, key: &str) -> PathBuf {
        self.cfg()
            .cache_dir
            .join("workspaces")
            .join(key)
            .join("release/deps/cache.bin")
    }
    fn source(&self, name: &str) -> PathBuf {
        let path = self.path(name);
        fs::create_dir_all(path.join("src")).unwrap();
        path
    }
}
fn success(output: &Output) {
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn registry_install_outside_workspace_has_explicit_identity_and_managed_config() {
    let f = Fixture::new();
    success(&f.run(&[
        "install",
        "demo",
        "--version",
        "=1.2.3",
        "--registry",
        "private",
        "--locked",
        "--offline",
    ]));
    let store = f.store();
    let entries = store.entries().unwrap();
    assert_eq!(entries.len(), 1);
    let record = &entries[0].record;
    assert_eq!(record.kind, CacheKind::Install);
    assert!(record.workspace.as_os_str().is_empty());
    assert_eq!(record.install_identity.as_ref().unwrap().len(), 64);
    assert!(f.cached(&record.key).is_file());
    let trace = fs::read_to_string(f.path("trace")).unwrap();
    assert!(!trace.contains("locate-project"));
    let args = fs::read_to_string(f.path("args")).unwrap();
    assert!(args.contains("build.build-dir="));
    for value in ["demo", "=1.2.3", "private", "--offline", "--locked"] {
        assert!(args.contains(value));
    }
}

#[test]
fn git_urls_and_options_forward_without_secrets_in_metadata_logs_or_wrapper_output() {
    let f = Fixture::new();
    let secret = "DO-NOT-PERSIST-THIS";
    let url = format!("https://user:{secret}@example.invalid/repo?token={secret}");
    let output = f.run(&[
        "install",
        "--git",
        &url,
        "--rev",
        "abc",
        "--bin",
        "demo",
        "--features",
        "a,b",
    ]);
    success(&output);
    assert!(fs::read_to_string(f.path("args")).unwrap().contains(&url));
    for file in [
        f.cfg()
            .cache_dir
            .join("state")
            .join(format!("{}.json", f.key())),
        f.path("events.jsonl"),
    ] {
        let value = fs::read_to_string(file).unwrap();
        assert!(!value.contains(secret));
        assert!(!value.contains("example.invalid"));
    }
    assert!(!String::from_utf8_lossy(&output.stdout).contains(secret));
    assert!(!String::from_utf8_lossy(&output.stderr).contains(secret));
}

#[test]
fn path_aliases_share_identity_but_different_sources_and_toolchains_do_not() {
    let f = Fixture::new();
    let source = f.source("source with spaces");
    success(&f.run(&["+stable", "install", "--path", source.to_str().unwrap()]));
    let key = f.key();
    let equivalent = format!(
        "--path={}/./",
        cargoc::util::identity_path(&source).unwrap()
    );
    success(&f.run(&["+stable", "install", &equivalent]));
    assert_eq!(f.store().entries().unwrap().len(), 1);
    assert_eq!(f.key(), key);
    success(&f.run(&["+stable", "install", "--path", "../source with spaces"]));
    assert_eq!(f.store().entries().unwrap().len(), 1);
    success(&f.run(&["+nightly", "install", &equivalent]));
    let second = f.source("different source");
    success(&f.run(&["+stable", "install", "--path", second.to_str().unwrap()]));
    assert_eq!(f.store().entries().unwrap().len(), 3);
    let output = f
        .command()
        .args(["exec", "+stable", "install", "--path"])
        .arg(&source)
        .env("INSTALL_CARGO_VERSION", "1.99.1")
        .output()
        .unwrap();
    success(&output);
    assert_eq!(f.store().entries().unwrap().len(), 4);
}

#[test]
fn registry_versions_indexes_and_git_revisions_are_isolated() {
    let f = Fixture::new();
    for args in [
        vec!["install", "demo", "--version", "1"],
        vec!["install", "demo", "--version", "2"],
        vec![
            "install",
            "demo",
            "--version",
            "1",
            "--index",
            "https://example.invalid/registry",
        ],
        vec![
            "install",
            "--git",
            "https://example.invalid/repo",
            "--rev",
            "one",
        ],
        vec![
            "install",
            "--git",
            "https://example.invalid/repo",
            "--rev",
            "two",
        ],
    ] {
        success(&f.run(&args));
    }
    assert_eq!(f.store().entries().unwrap().len(), 5);
}

#[test]
fn leading_common_globals_and_toolchain_do_not_hide_install() {
    let f = Fixture::new();
    let output = f.run(&[
        "+stable",
        "--color",
        "never",
        "-vv",
        "--locked",
        "--config",
        "net.offline=true",
        "-Z",
        "unstable-options",
        "install",
        "demo",
    ]);
    success(&output);
    let args = fs::read_to_string(f.path("args")).unwrap();
    for value in [
        "+stable",
        "never",
        "-vv",
        "--locked",
        "net.offline=true",
        "unstable-options",
        "install",
        "demo",
    ] {
        assert!(args.contains(value));
    }
    assert!(
        !fs::read_to_string(f.path("trace"))
            .unwrap()
            .contains("locate-project")
    );
    assert_eq!(
        f.store().entries().unwrap()[0].record.kind,
        CacheKind::Install
    );
}

#[test]
fn list_help_and_help_install_are_true_passthrough_even_on_old_cargo() {
    for args in [
        vec!["install", "--list"],
        vec!["install", "--help"],
        vec!["install", "-h"],
        vec!["install", "--path", "--help"],
        vec!["--color", "never", "--help", "install"],
        vec!["help", "install"],
        vec![
            "+old",
            "--color=never",
            "--config",
            "net.offline=true",
            "install",
            "--list",
        ],
    ] {
        let f = Fixture::new();
        let output = f
            .command()
            .arg("exec")
            .args(&args)
            .env("INSTALL_CARGO_VERSION", "1.50.0")
            .output()
            .unwrap();
        success(&output);
        assert!(!f.cfg().cache_dir.exists());
        assert!(!f.path("events.jsonl").exists());
        let trace = fs::read_to_string(f.path("trace")).unwrap();
        assert!(!trace.contains("--version"));
        assert!(!trace.contains("locate-project"));
        assert!(!trace.contains("build.build-dir"));
    }
}

#[test]
fn install_requires_new_cargo_without_locating_workspace() {
    let f = Fixture::new();
    let output = f
        .command()
        .args(["exec", "install", "demo"])
        .env("INSTALL_CARGO_VERSION", "1.90.0")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Cargo >= 1.91"));
    assert!(!f.cfg().cache_dir.exists());
    assert!(
        !fs::read_to_string(f.path("trace"))
            .unwrap()
            .contains("locate-project")
    );
}

#[test]
fn build_dir_overrides_are_rejected_but_target_dir_and_native_flags_forward() {
    let f = Fixture::new();
    for args in [
        vec!["install", "demo", "--build-dir=elsewhere"],
        vec!["install", "demo", "--config", "build.build-dir='elsewhere'"],
        vec!["--config=build={build-dir='elsewhere'}", "install", "demo"],
    ] {
        assert!(!f.run(&args).status.success());
        assert!(!f.path("started").exists());
    }
    let output = f.run(&[
        "install",
        "demo",
        "--target-dir",
        "final target",
        "--root",
        "install root",
        "--force",
        "--no-default-features",
        "--",
        "--build-dir=package-name",
    ]);
    success(&output);
    let args = fs::read_to_string(f.path("args")).unwrap();
    for flag in [
        "final target",
        "install root",
        "--force",
        "--no-default-features",
        "--build-dir=package-name",
    ] {
        assert!(args.contains(flag));
    }
}

#[test]
fn same_install_lease_rejects_concurrency_and_manual_clean_respects_it() {
    let f = Fixture::new();
    let mut child = f
        .command()
        .args(["exec", "install", "demo"])
        .env("INSTALL_MODE", "wait")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !f.path("started").exists() {
        assert!(Instant::now() < deadline);
        assert!(child.try_wait().unwrap().is_none());
        thread::sleep(Duration::from_millis(20));
    }
    let key = f.key();
    assert!(f.store().lease(&key).unwrap().is_none());
    let output = f.run(&["install", "demo"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("busy"));
    let report = f
        .store()
        .collect(&GcOptions {
            all_idle: true,
            ..GcOptions::default()
        })
        .unwrap();
    assert_eq!(report.removed_bytes, 0);
    assert!(!report.skipped.is_empty());
    assert!(f.cached(&key).is_file());
    success(&f.run(&["install", "different-demo"]));
    assert!(child.wait().unwrap().success());
    let output = f.command().arg("clean").output().unwrap();
    success(&output);
    assert!(!f.cached(&key).exists());
    assert!(f.path("installed/bin/installed").is_file());
}

#[test]
fn install_native_lock_protects_cache_and_manual_clean_never_removes_bins() {
    let f = Fixture::new();
    success(&f.run(&["install", "demo"]));
    let key = f.key();
    let cached = f.cached(&key);
    let lock = Lease::acquire(
        &cached
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join(".cargo-lock"),
        false,
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        f.store()
            .collect(&GcOptions {
                all_idle: true,
                ..GcOptions::default()
            })
            .unwrap()
            .removed_bytes,
        0
    );
    drop(lock);
    assert_eq!(
        f.store()
            .collect(&GcOptions {
                all_idle: true,
                ..GcOptions::default()
            })
            .unwrap()
            .removed_bytes,
        8192
    );
    assert_eq!(
        fs::read_to_string(f.path("installed/bin/installed")).unwrap(),
        "final installed binary"
    );
}

#[test]
fn failure_keeps_native_exit_code_and_logs_only_opaque_subject() {
    let f = Fixture::new();
    let output = f
        .command()
        .args(["exec", "install", "demo"])
        .env("INSTALL_MODE", "fail")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(7));
    let log = fs::read_to_string(f.path("events.jsonl")).unwrap();
    assert!(log.contains("cargo-exit"));
    assert!(log.contains("\"exit_code\":7"));
    assert!(log.contains("\"install\""));
    assert!(f.store().lease(&f.key()).unwrap().is_some());
}

#[test]
fn cleanup_failure_does_not_mask_native_failure() {
    let f = Fixture::new();
    let output = f
        .command()
        .args(["exec", "install", "demo"])
        .env("INSTALL_MODE", "post-error")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(7));
    assert!(String::from_utf8_lossy(&output.stderr).contains("safe cleanup unavailable"));
    assert!(
        fs::read_to_string(f.path("events.jsonl"))
            .unwrap()
            .contains("cargo-exit")
    );
}

#[test]
fn disk_refusal_happens_before_start_and_releases_lease() {
    let f = Fixture::new();
    f.configure(|cfg| cfg.min_free_gib = u64::MAX as f64 / GIB / 2.0);
    let output = f.run(&["install", "demo"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Insufficient free disk"));
    assert!(!f.path("started").exists());
    assert!(f.store().lease(&f.key()).unwrap().is_some());
}

#[test]
fn post_install_gc_enforces_budget_without_touching_installed_bin() {
    let f = Fixture::new();
    f.configure(|cfg| cfg.max_cache_gib = 1024.0 / GIB);
    success(&f.run(&["install", "demo"]));
    assert!(!f.cached(&f.key()).exists());
    assert!(f.path("installed/bin/installed").is_file());
    assert!(
        fs::read_to_string(f.path("events.jsonl"))
            .unwrap()
            .contains("\"kind\":\"clean\"")
    );
}

#[test]
fn during_install_gc_preserves_active_cache_and_removes_idle_cache() {
    let f = Fixture::new();
    success(&f.run(&["install", "idle"]));
    let idle_key = f.key();
    f.configure(|cfg| cfg.max_cache_gib = 12_000.0 / GIB);
    let output = f
        .command()
        .args(["exec", "install", "active"])
        .env("INSTALL_MODE", "wait")
        .output()
        .unwrap();
    success(&output);
    assert!(!f.cached(&idle_key).exists());
    assert_eq!(
        f.store()
            .entries()
            .unwrap()
            .iter()
            .filter(|e| e.bytes > 0)
            .count(),
        1
    );
}

#[test]
fn legacy_workspace_records_and_new_install_records_coexist_and_reject_forgery() {
    let f = Fixture::new();
    success(&f.run(&["install", "demo"]));
    let store = f.store();
    let source = f.source("legacy-workspace");
    let (key, _) = store.touch(&source).unwrap();
    let path = store.root.join("state").join(format!("{key}.json"));
    let mut old: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    old.as_object_mut().unwrap().remove("kind");
    old.as_object_mut().unwrap().remove("install_identity");
    fs::write(&path, serde_json::to_vec(&old).unwrap()).unwrap();
    assert_eq!(store.entries().unwrap().len(), 2);
    let key = f.key(); // Whichever record is first, an inconsistent kind is invalid.
    let path = store.root.join("state").join(format!("{key}.json"));
    let mut data: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    data["kind"] = serde_json::json!("install");
    data["install_identity"] = serde_json::json!("0".repeat(64));
    fs::write(path, serde_json::to_vec(&data).unwrap()).unwrap();
    assert!(store.entries().is_err());
    assert!(
        store
            .collect(&GcOptions {
                all_idle: true,
                ..GcOptions::default()
            })
            .is_err()
    );
}

#[test]
fn real_offline_path_install_and_manual_clean_preserve_runnable_binary() {
    let f = Fixture::new();
    let cargo = std::env::var_os("CARGO")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("cargo"));
    let rustup_home = std::env::var_os("RUSTUP_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| cargoc::config::home().join(".rustup"));
    let output = Command::new(&cargo).arg("--version").output().unwrap();
    success(&output);
    assert!(
        cargoc::cargo::version_number(&String::from_utf8_lossy(&output.stdout)).unwrap() >= (1, 91)
    );
    f.configure(|cfg| cfg.cargo = cargo);
    let source = f.source("real source");
    fs::write(
        source.join("Cargo.toml"),
        "[package]\nname='cargoc-install-smoke'\nversion='0.1.0'\nedition='2021'\n[workspace]\n",
    )
    .unwrap();
    fs::write(
        source.join("Cargo.lock"),
        "version = 4\n\n[[package]]\nname = \"cargoc-install-smoke\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    fs::write(
        source.join("src/main.rs"),
        "fn main() { println!(\"installed and alive\"); }\n",
    )
    .unwrap();
    let root = f.path("real-installed");
    let target = f.path("final-target");
    let output = f
        .command()
        .args(["exec", "install", "--path"])
        .arg(&source)
        .arg("--root")
        .arg(&root)
        .arg("--target-dir")
        .arg(&target)
        .args(["--offline", "--locked"])
        .env("RUSTUP_HOME", rustup_home)
        .env("RUSTUP_AUTO_INSTALL", "0")
        .output()
        .unwrap();
    success(&output);
    let binary = root.join("bin").join(format!(
        "cargoc-install-smoke{}",
        std::env::consts::EXE_SUFFIX
    ));
    let before = Command::new(&binary).output().unwrap();
    success(&before);
    assert_eq!(
        String::from_utf8_lossy(&before.stdout).trim(),
        "installed and alive"
    );
    assert!(
        target
            .join("release")
            .join(binary.file_name().unwrap())
            .is_file()
    );
    let entries = f.store().entries().unwrap();
    assert_eq!(entries.len(), 1);
    assert!(entries[0].bytes > 0);
    let output = f.command().arg("clean").output().unwrap();
    success(&output);
    assert_eq!(f.store().entries().unwrap()[0].bytes, 0);
    let after = Command::new(&binary).output().unwrap();
    success(&after);
    assert_eq!(after.stdout, before.stdout);
    assert!(source.join("src/main.rs").is_file());
    assert!(
        target
            .join("release")
            .join(binary.file_name().unwrap())
            .is_file()
    );
}

#[test]
fn pre_install_gc_runs_before_cargo_and_no_clean_remains_opt_in() {
    let f = Fixture::new();
    success(&f.run(&["install", "idle"]));
    let idle = f.cached(&f.key());
    f.configure(|cfg| cfg.max_cache_gib = 1024.0 / GIB);
    let output = f
        .command()
        .args(["exec", "install", "active"])
        .env("INSTALL_EXPECT_REMOVED", &idle)
        .output()
        .unwrap();
    success(&output);
    assert!(!idle.exists());
    f.configure(|cfg| cfg.auto_clean = false);
    success(&f.run(&["install", "keep-oversize"]));
    assert!(f.store().entries().unwrap().iter().any(|e| e.bytes == 8192));
    let output = f.command().args(["clean", "--dry-run"]).output().unwrap();
    success(&output);
    assert!(f.store().entries().unwrap().iter().any(|e| e.bytes == 8192));
    success(&f.command().arg("clean").output().unwrap());
    assert!(f.store().entries().unwrap().iter().all(|e| e.bytes == 0));
    assert!(f.path("installed/bin/installed").is_file());
}

#[test]
fn inherited_unmanaged_build_dir_is_overridden_and_argument_os_bytes_survive() {
    let f = Fixture::new();
    #[cfg(windows)]
    let unusual = {
        use std::os::windows::ffi::OsStringExt;
        std::ffi::OsString::from_wide(&[b'f' as u16, 0xd800, b'x' as u16])
    };
    #[cfg(not(windows))]
    let unusual = {
        use std::os::unix::ffi::OsStringExt;
        std::ffi::OsString::from_vec(vec![b'f', 0xff, b'x'])
    };
    let output = f
        .command()
        .args(["exec", "install", "demo", "--features"])
        .arg(&unusual)
        .args(["--", "--path"])
        .env("CARGO_BUILD_BUILD_DIR", f.path("unmanaged-build"))
        .output()
        .unwrap();
    success(&output);
    assert!(!f.path("unmanaged-build").exists());
    assert_eq!(f.store().entries().unwrap()[0].bytes, 8192);
    assert!(
        fs::read_to_string(f.path("args"))
            .unwrap()
            .contains(&format!("{unusual:?}"))
    );
}

#[test]
fn validation_errors_do_not_echo_secret_arguments_or_paths() {
    let f = Fixture::new();
    for args in [
        vec!["install", "--path", "NONEXISTENT-SECRET-SOURCE"],
        vec!["install", "demo", "--config", "secret='SECRET-BROKEN-TOML"],
        vec!["install", "--path"],
    ] {
        let output = f.run(&args);
        assert!(!output.status.success());
        assert!(!String::from_utf8_lossy(&output.stderr).contains("SECRET"));
        assert!(!f.cfg().cache_dir.exists());
        assert!(!f.path("events.jsonl").exists());
    }
}

#[test]
fn install_sources_and_final_outputs_cannot_be_placed_in_disposable_cache() {
    let f = Fixture::new();
    let cache = f.cfg().cache_dir;
    let destination = cache.join("workspaces/old-cache/bin");
    for flag in ["--root", "--target-dir", "--path"] {
        let output = f
            .command()
            .args(["exec", "install", flag])
            .arg(&destination)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("outside the managed cache root"));
        assert!(!cache.exists());
    }
    for variable in ["CARGO_INSTALL_ROOT", "CARGO_HOME", "CARGO_TARGET_DIR"] {
        let output = f
            .command()
            .args(["exec", "install", "demo"])
            .env(variable, &destination)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("outside the managed cache root"));
        assert!(!cache.exists());
    }
    let escaped = toml::Value::String(destination.to_string_lossy().into_owned());
    for setting in [
        format!("install.root={escaped}"),
        format!("build.target-dir={escaped}"),
    ] {
        let output = f
            .command()
            .args(["exec", "install", "demo", "--config"])
            .arg(setting)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("managed cache root"));
        assert!(!cache.exists());
    }
    let config_source = f.path("unsafe-cargo-config.toml");
    fs::write(&config_source, format!("install.root = {escaped}\n")).unwrap();
    let output = f
        .command()
        .args(["exec", "install", "demo", "--config"])
        .arg(&config_source)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("managed cache root"));
    assert!(!cache.exists());
}

#[test]
fn config_file_forms_preserve_safe_outputs_and_resolve_relative_unsafe_paths() {
    let f = Fixture::new();
    let config_source = f.path("extra.toml");
    fs::write(&config_source, "[net]\noffline = true\n").unwrap();
    let attached = format!("--config={}", config_source.display());
    success(&f.run(&["install", "demo", &attached]));
    success(&f.run(&[
        "install",
        "demo",
        "--config",
        config_source.to_str().unwrap(),
    ]));
    let config_dir = f.path(".cargo");
    fs::create_dir_all(&config_dir).unwrap();
    let config_source = config_dir.join("extra.toml");
    fs::write(
        &config_source,
        "[install]\nroot = 'cache/workspaces/old/bin'\n",
    )
    .unwrap();
    for args in [
        vec![
            "--config".to_owned(),
            config_source.to_string_lossy().into_owned(),
        ],
        vec![format!("--config={}", config_source.display())],
    ] {
        let output = f
            .command()
            .args(["exec", "install", "demo"])
            .args(args)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("inside the managed cache root"));
    }
}

#[test]
fn status_labels_install_records_without_exposing_source_arguments() {
    let f = Fixture::new();
    success(&f.run(&["install", "private-package-name"]));
    let output = f.command().arg("status").output().unwrap();
    success(&output);
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains(&format!("install {}", f.key())));
    assert!(!text.contains("private-package-name"));
    let output = f.command().args(["status", "--json"]).output().unwrap();
    success(&output);
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["workspaces"][0]["record"]["kind"], "install");
}
