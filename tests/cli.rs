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
