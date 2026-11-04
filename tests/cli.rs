use cargoc::{config::Config, store::Store};
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output, Stdio},
    sync::OnceLock,
    thread,
    time::{Duration, Instant},
};
use tempfile::TempDir;

fn temp() -> TempDir {
    tempfile::tempdir_in(fs::canonicalize(std::env::temp_dir()).unwrap()).unwrap()
}

fn fake_cargo() -> &'static PathBuf {
    static COMPILED: OnceLock<(TempDir, PathBuf)> = OnceLock::new();
    &COMPILED
        .get_or_init(|| {
            let dir = temp();
            let out = dir
                .path()
                .join(format!("fake-cargo{}", std::env::consts::EXE_SUFFIX));
            let source =
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fake_cargo.rs");
            let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
            let output = Command::new(rustc)
                .arg("--edition=2021")
                .arg(source)
                .arg("-o")
                .arg(&out)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            (dir, out)
        })
        .1
}

struct Fixture {
    _temp: TempDir,
    config: PathBuf,
    workspace: PathBuf,
    arguments: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let temp = temp();
        let workspace = temp.path().join("workspace with spaces");
        fs::create_dir_all(&workspace).unwrap();
        fs::write(workspace.join("Cargo.toml"), "source").unwrap();
        let config_path = temp.path().join("config.toml");
        let config = Config {
            cache_dir: temp.path().join("cache"),
            cargo: fake_cargo().clone(),
            min_free_gib: 0.0,
            abort_free_gib: 0.0,
            max_age_days: 0.0,
            poll_seconds: 0.1,
            ..Config::default()
        };
        config.save(&config_path).unwrap();
        Self {
            config: config_path,
            workspace,
            arguments: temp.path().join("arguments.json"),
            _temp: temp,
        }
    }
    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cargoc"));
        command
            .arg("--cargoc-config")
            .arg(&self.config)
            .current_dir(&self.workspace)
            .env("HOME", self._temp.path())
            .env("USERPROFILE", self._temp.path())
            .env("CARGO_HOME", self._temp.path().join("cargo-home"))
            .env(
                "RUSTUP_HOME",
                std::env::var_os("RUSTUP_HOME")
                    .unwrap_or_else(|| cargoc::config::home().join(".rustup").into_os_string()),
            )
            .env("FAKE_WORKSPACE", &self.workspace)
            .env("FAKE_MANIFEST", self.workspace.join("Cargo.toml"))
            .env("FAKE_ARGUMENTS", &self.arguments);
        command
    }
    fn run(&self, args: &[&str]) -> Output {
        self.command().args(args).output().unwrap()
    }
    fn cache_file(&self) -> PathBuf {
        let cfg = Config::load(&self.config).unwrap();
        cfg.cache_dir
            .join("workspaces")
            .join(Store::key(&self.workspace).unwrap())
            .join("debug/deps/cache.bin")
    }
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn cargo_version_shows_both_versions_and_preserves_verbose_metadata() {
    let f = Fixture::new();
    for args in [
        vec!["exec", "--version"],
        vec!["exec", "-V"],
        vec!["exec", "-vV"],
        vec!["exec", "-Vv"],
        vec!["exec", "--verbose", "--version"],
        vec!["version"],
        vec!["exec", "+stable", "--version"],
    ] {
        let output = f.run(&args);
        assert_success(&output);
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(text.starts_with("cargo 1.99.0 (test)\n"), "{text}");
        assert!(
            text.ends_with(&format!("cargoc {}\n", env!("CARGO_PKG_VERSION"))),
            "{text}"
        );
        assert!(!String::from_utf8_lossy(&output.stderr).contains("passthrough"));
        if args
            .iter()
            .any(|s| matches!(*s, "-vV" | "-Vv" | "--verbose"))
        {
            assert!(text.contains("release: 1.99.0\nlibgit2: test\n"));
        }
        assert!(!f.cache_file().exists());
    }
    let recorded = f.workspace.join("version-args.json");
    let output = f
        .command()
        .args(["exec", "+nightly", "--version", "--verbose"])
        .env("FAKE_VERSION_ARGUMENTS", &recorded)
        .output()
        .unwrap();
    assert_success(&output);
    let args: Vec<String> = serde_json::from_slice(&fs::read(recorded).unwrap()).unwrap();
    assert_eq!(args, ["+nightly", "--version", "--verbose"]);
}

#[test]
fn version_failure_preserves_cargo_error_without_success_banner() {
    let f = Fixture::new();
    let output = f
        .command()
        .args(["exec", "--version"])
        .env("FAKE_MODE", "version-fail")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(7));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("native version failure"));
}

#[test]
fn path_discovery_skips_generated_shims_and_keeps_directory_priority() {
    let f = Fixture::new();
    let shim = f.workspace.join("shim");
    let first = f.workspace.join("first");
    let second = f.workspace.join("second");
    for dir in [&shim, &first, &second] {
        fs::create_dir_all(dir).unwrap();
    }
    let name = format!("cargo{}", std::env::consts::EXE_SUFFIX);
    let first_cargo = first.join(&name);
    fs::copy(fake_cargo(), &first_cargo).unwrap();
    fs::copy(fake_cargo(), second.join(&name)).unwrap();
    let shim_path = shim.join(if cfg!(windows) { "cargo.cmd" } else { "cargo" });
    fs::write(&shim_path, "#!/bin/sh\n# Generated by cargoc.\nexit 99\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&shim_path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    // A later Git Bash script must not beat an earlier native Windows executable.
    #[cfg(windows)]
    fs::write(second.join("cargo"), "not a Windows executable").unwrap();
    let recorded = f.workspace.join("version-executable.txt");
    let mut cfg = Config::load(&f.config).unwrap();
    cfg.cargo = "cargo".into();
    cfg.save(&f.config).unwrap();
    let output = f
        .command()
        .args(["exec", "--version"])
        .env(
            "PATH",
            std::env::join_paths([&shim, &first, &second]).unwrap(),
        )
        .env("PATHEXT", ".COM;.EXE;.BAT;.CMD")
        .env("FAKE_VERSION_EXECUTABLE", &recorded)
        .output()
        .unwrap();
    assert_success(&output);
    let launched = fs::read_to_string(recorded).unwrap();
    assert_eq!(
        cargoc::util::identity_path(std::path::Path::new(&launched)).unwrap(),
        cargoc::util::identity_path(&first_cargo).unwrap()
    );
}

#[test]
fn explicit_shim_and_copied_wrapper_are_rejected_before_setup_changes() {
    let f = Fixture::new();
    let shim = f.workspace.join("foreign-cargo.cmd");
    fs::write(&shim, "@echo off\r\nrem Generated by cargoc.\r\n").unwrap();
    let copy = f
        .workspace
        .join(format!("copied-cargoc{}", std::env::consts::EXE_SUFFIX));
    fs::copy(env!("CARGO_BIN_EXE_cargoc"), &copy).unwrap();
    let original = fs::read(&f.config).unwrap();
    for invalid in [&shim, &copy] {
        let output = f
            .command()
            .args(["setup", "--enable", "--cargo"])
            .arg(invalid)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
        assert_eq!(fs::read(&f.config).unwrap(), original);
        assert!(!f.workspace.join("started").exists());
    }
    let output = f
        .command()
        .args(["setup", "--enable", "--cargo"])
        .arg(fake_cargo())
        .env("FAKE_MODE", "not-cargo")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(fs::read(&f.config).unwrap(), original);
}

#[test]
fn explicit_generated_shim_is_rejected_at_runtime_and_depth_is_bounded() {
    let f = Fixture::new();
    let shim = f.workspace.join("cargo-shim.cmd");
    fs::write(&shim, "@echo off\r\nrem Generated by cargoc.\r\n").unwrap();
    let mut cfg = Config::load(&f.config).unwrap();
    cfg.cargo = shim;
    cfg.save(&f.config).unwrap();
    let output = f.run(&["exec", "--version"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("points back"));
    cfg.cargo = fake_cargo().clone();
    cfg.save(&f.config).unwrap();
    let output = f
        .command()
        .args(["exec", "--version"])
        .env("CARGOC_FORWARD_DEPTH", "8")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("recursion"));
}

#[test]
fn setup_refuses_unowned_shim_before_any_config_or_path_change() {
    let f = Fixture::new();
    let home = f.workspace.join("isolated-home");
    let bin = home.join(".cargoc/bin");
    fs::create_dir_all(&bin).unwrap();
    let path = bin.join(if cfg!(windows) { "cargo.cmd" } else { "cargo" });
    fs::write(&path, "user-owned command, do not overwrite").unwrap();
    let original = fs::read(&f.config).unwrap();
    let output = f
        .command()
        .args(["setup", "--enable", "--cargo"])
        .arg(fake_cargo())
        .env("HOME", &home)
        .env("USERPROFILE", &home)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("unowned shim"));
    assert_eq!(fs::read(&f.config).unwrap(), original);
    assert_eq!(
        fs::read_to_string(path).unwrap(),
        "user-owned command, do not overwrite"
    );
    assert!(!home.join(".cargoc/integration.toml").exists());
}

#[cfg(unix)]
#[test]
fn rustup_symlink_is_launched_with_the_cargo_invocation_name() {
    let f = Fixture::new();
    let rustup = f.workspace.join("rustup");
    fs::copy(fake_cargo(), &rustup).unwrap();
    let proxy = f.workspace.join("cargo");
    std::os::unix::fs::symlink(&rustup, &proxy).unwrap();
    let mut cfg = Config::load(&f.config).unwrap();
    cfg.cargo = proxy;
    cfg.save(&f.config).unwrap();
    let output = f
        .command()
        .args(["exec", "--version"])
        .env("FAKE_REQUIRE_CARGO_NAME", "1")
        .output()
        .unwrap();
    assert_success(&output);
    assert!(String::from_utf8_lossy(&output.stdout).starts_with("cargo 1.99.0 (test)\n"));
}

#[test]
fn status_does_not_report_deleted_shims_as_active() {
    let f = Fixture::new();
    let home = f.workspace.join("isolated-status-home");
    fs::create_dir_all(home.join(".cargoc")).unwrap();
    let state = cargoc::integration::IntegrationState {
        shim_dir: home.join(".cargoc/bin"),
        real_cargo: fake_cargo().clone(),
        enabled: true,
        config_path: Some(f.config.clone()),
    };
    fs::write(
        home.join(".cargoc/integration.toml"),
        toml::to_string(&state).unwrap(),
    )
    .unwrap();
    let output = f
        .command()
        .args(["setup", "--status"])
        .env("HOME", &home)
        .env("USERPROFILE", &home)
        .output()
        .unwrap();
    assert_success(&output);
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(
        text.contains("shim_installed=false active_in_current_path=false"),
        "{text}"
    );
    assert!(
        text.contains("real_cargo_valid=true config_valid=true"),
        "{text}"
    );
}

#[test]
fn arguments_config_quotes_and_final_artifacts_are_preserved() {
    let f = Fixture::new();
    assert_success(&f.run(&[
        "run",
        "--release",
        "--",
        "argument with spaces",
        "--config",
        "application value",
    ]));
    let args: Vec<String> = serde_json::from_slice(&fs::read(&f.arguments).unwrap()).unwrap();
    assert_eq!(args[0], "--config");
    let injected: toml::Value = toml::from_str(&args[1]).unwrap();
    let cfg = Config::load(&f.config).unwrap();
    let expected = cfg
        .cache_dir
        .join("workspaces")
        .join(Store::key(&f.workspace).unwrap());
    assert_eq!(
        PathBuf::from(injected["build"]["build-dir"].as_str().unwrap()),
        expected
    );
    assert_eq!(
        &args[2..],
        [
            "run",
            "--release",
            "--",
            "argument with spaces",
            "--config",
            "application value"
        ]
    );
    assert!(f.cache_file().is_file());
    assert!(f.workspace.join("target/debug/demo.exe").is_file());
    assert_success(&f.run(&["clean", "--dry-run"]));
    assert!(f.cache_file().exists());
    assert_success(&f.run(&["clean"]));
    assert!(!f.cache_file().exists());
    assert_eq!(
        fs::read_to_string(f.workspace.join("Cargo.toml")).unwrap(),
        "source"
    );
    assert_eq!(
        fs::read_to_string(f.workspace.join("target/debug/demo.exe")).unwrap(),
        "final artifact"
    );
}

#[test]
fn cargo_failure_exit_code_is_preserved() {
    let f = Fixture::new();
    let output = f
        .command()
        .arg("test")
        .env("FAKE_MODE", "fail")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(7));
}

#[test]
fn maintenance_failure_does_not_overwrite_cargo_exit_code() {
    let f = Fixture::new();
    let output = f
        .command()
        .arg("check")
        .env("FAKE_MODE", "post-gc-error")
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(7),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("cleanup unavailable"));
}

#[test]
fn low_space_refuses_to_launch_without_filling_disk() {
    let f = Fixture::new();
    let mut cfg = Config::load(&f.config).unwrap();
    cfg.min_free_gib = 1_000_000_000.0;
    cfg.save(&f.config).unwrap();
    let output = f.run(&["build"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(!f.workspace.join("started").exists());
}

#[test]
fn running_build_is_protected_then_its_oversize_cache_is_collected_after_exit() {
    let f = Fixture::new();
    let mut cfg = Config::load(&f.config).unwrap();
    cfg.max_cache_gib = 0.000_001;
    cfg.save(&f.config).unwrap();
    let child = f
        .command()
        .arg("build")
        .env("FAKE_MODE", "wait")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let start = Instant::now();
    while !f.workspace.join("started").exists() {
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "build never started"
        );
        thread::sleep(Duration::from_millis(25));
    }
    let clean = f.run(&["clean"]);
    assert_success(&clean);
    let report: serde_json::Value = serde_json::from_slice(&clean.stdout).unwrap();
    assert_eq!(report["removed_bytes"], 0);
    assert!(!report["skipped"].as_array().unwrap().is_empty());
    assert!(f.cache_file().is_file());
    let output = child.wait_with_output().unwrap();
    assert_success(&output); // Fake compiler fails if its live cache disappears.
    assert!(!f.cache_file().exists());
    assert!(f.workspace.join("target/debug/demo.exe").is_file());
}

#[test]
fn recursive_cargo_configuration_is_rejected() {
    let f = Fixture::new();
    let mut cfg = Config::load(&f.config).unwrap();
    cfg.cargo = PathBuf::from(env!("CARGO_BIN_EXE_cargoc"));
    cfg.save(&f.config).unwrap();
    let output = f.run(&["fmt"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("points back"));
}

#[test]
fn configuration_cli_initializes_and_updates_without_adopting_old_roots() {
    let temp = temp();
    let config_path = temp.path().join("user/config.toml");
    let root = temp.path().join("owned cache");
    let mut init = Command::new(env!("CARGO_BIN_EXE_cargoc"));
    let output = init
        .arg("--cargoc-config")
        .arg(&config_path)
        .arg("init")
        .arg("--cache-dir")
        .arg(&root)
        .args(["--max-cache-gib", "42", "--auto-clean", "false"])
        .output()
        .unwrap();
    assert_success(&output);
    let cfg = Config::load(&config_path).unwrap();
    assert_eq!(cfg.max_cache_gib, 42.0);
    assert!(!cfg.auto_clean);
    assert_success(
        &Command::new(env!("CARGO_BIN_EXE_cargoc"))
            .arg("--cargoc-config")
            .arg(&config_path)
            .args(["config", "--max-cache-gib", "24"])
            .output()
            .unwrap(),
    );
    assert_eq!(Config::load(&config_path).unwrap().max_cache_gib, 24.0);
    let old = temp.path().join("old-cache");
    fs::create_dir_all(&old).unwrap();
    fs::write(old.join("do-not-delete"), "old cache").unwrap();
    let failed = Command::new(env!("CARGO_BIN_EXE_cargoc"))
        .arg("--cargoc-config")
        .arg(&config_path)
        .args(["config", "--cache-dir"])
        .arg(&old)
        .output()
        .unwrap();
    assert_eq!(failed.status.code(), Some(2));
    assert_eq!(Config::load(&config_path).unwrap().cache_dir, root);
    assert!(old.join("do-not-delete").exists());
}

#[test]
fn real_cargo_build_and_clean_leave_the_final_binary_runnable() {
    let f = Fixture::new();
    fs::create_dir_all(f.workspace.join("src")).unwrap();
    fs::write(
        f.workspace.join("Cargo.toml"),
        "[package]\nname='real-cargoc-smoke'\nversion='0.1.0'\nedition='2021'\n[workspace]\n",
    )
    .unwrap();
    fs::write(
        f.workspace.join("src/main.rs"),
        "fn main() { println!(\"real smoke OK\"); }\n",
    )
    .unwrap();
    let mut config = Config::load(&f.config).unwrap();
    config.cargo = std::env::var_os("CARGO")
        .unwrap_or_else(|| "cargo".into())
        .into();
    config.save(&f.config).unwrap();
    let target = f.workspace.join("target");
    let output = f
        .command()
        .args(["build", "--offline", "--target-dir"])
        .arg(&target)
        .env_remove("CARGO_TARGET_DIR")
        .env_remove("RUSTC_WRAPPER")
        .env_remove("RUSTC_WORKSPACE_WRAPPER")
        .output()
        .unwrap();
    assert_success(&output);
    let store = Store::open(config, false).unwrap();
    assert!(store.entries().unwrap().iter().any(|e| e.bytes > 0));
    assert_success(&f.run(&["clean"]));
    let binary = target
        .join("debug")
        .join(format!("real-cargoc-smoke{}", std::env::consts::EXE_SUFFIX));
    let output = Command::new(binary).output().unwrap();
    assert_success(&output);
    assert!(String::from_utf8_lossy(&output.stdout).contains("real smoke OK"));
    assert!(f.workspace.join("src/main.rs").is_file());
}
