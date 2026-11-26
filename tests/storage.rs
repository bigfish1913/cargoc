use cargoc::{config::Config, storage, store::Store};
use std::fs;

#[test]
fn old_configuration_has_no_guard_and_round_trips() {
    let cfg: Config = toml::from_str("max_cache_gib = 42.0").unwrap();
    assert!(cfg.require_mount.is_none());
    assert!(cfg.required_root_id.is_none());
    assert!(cfg.clean_roots.is_empty());
    cfg.validate().unwrap();
    let text = toml::to_string(&cfg).unwrap();
    assert!(!text.contains("required_root_id"));
    assert!(!text.contains("require_mount"));
}

#[test]
fn unavailable_or_fake_mount_never_initializes_cache_or_locks() {
    let temp = tempfile::tempdir().unwrap();
    let parent = fs::canonicalize(temp.path()).unwrap();
    let mount = parent.join("missing-volume");
    let cfg = Config {
        cache_dir: mount.join("cache"),
        require_mount: Some(mount.clone()),
        required_root_id: Some("a".repeat(64)),
        ..Config::default()
    };
    // Configuration remains readable when a volume is disconnected.
    let config_path = parent.join("config.toml");
    cfg.save(&config_path).unwrap_or_else(|e| {
        if cfg!(unix) {
            panic!("{e}");
        }
    });
    assert!(Store::open(cfg.clone(), true).is_err());
    assert!(!mount.exists());
    fs::create_dir(&mount).unwrap();
    assert!(Store::open(cfg, true).is_err());
    assert_eq!(fs::read_dir(&mount).unwrap().count(), 0);
}

#[test]
fn binding_an_ordinary_directory_fails_without_mutating_config() {
    let temp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(temp.path()).unwrap();
    let mut cfg = Config {
        cache_dir: root.join("cache"),
        ..Config::default()
    };
    Store::open(cfg.clone(), true).unwrap();
    let before = fs::read(cfg.cache_dir.join(".cargoc-root.json")).unwrap();
    assert!(storage::bind(&mut cfg, &root).is_err());
    assert!(cfg.require_mount.is_none());
    assert!(cfg.required_root_id.is_none());
    assert_eq!(
        fs::read(cfg.cache_dir.join(".cargoc-root.json")).unwrap(),
        before
    );
}

#[test]
fn invalid_guard_combinations_are_rejected() {
    let mut cfg = Config {
        required_root_id: Some("a".repeat(64)),
        ..Config::default()
    };
    assert!(cfg.validate().is_err());
    cfg.require_mount = Some(cfg.cache_dir.clone());
    assert!(cfg.validate().is_err());
    cfg.require_mount = Some("/".into());
    assert!(cfg.validate().is_err());
}

#[cfg(unix)]
#[test]
fn guard_never_follows_symlinks() {
    let temp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(temp.path()).unwrap();
    let volume = root.join("volume");
    fs::create_dir(&volume).unwrap();
    let alias = root.join("alias");
    std::os::unix::fs::symlink(&volume, &alias).unwrap();
    let cfg = Config {
        cache_dir: alias.join("cache"),
        require_mount: Some(alias),
        required_root_id: Some("a".repeat(64)),
        ..Config::default()
    };
    assert!(Store::open(cfg, true).is_err());
    assert_eq!(fs::read_dir(volume).unwrap().count(), 0);
}

#[test]
fn configuration_cli_saves_clean_roots_without_changing_cache_identity() {
    let temp = tempfile::tempdir_in(fs::canonicalize(std::env::temp_dir()).unwrap()).unwrap();
    let cfg = Config {
        cache_dir: temp.path().join("cache"),
        ..Config::default()
    };
    Store::open(cfg.clone(), true).unwrap();
    let id = storage::root_id(&cfg.cache_dir).unwrap();
    let path = temp.path().join("config.toml");
    cfg.save(&path).unwrap();
    let root_a = temp.path().join("a");
    let root_b = temp.path().join("b");
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_cargoc"))
        .arg("--cargoc-config")
        .arg(&path)
        .args(["config", "--clean-root"])
        .arg(&root_a)
        .arg("--clean-root")
        .arg(&root_b)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        Config::load(&path).unwrap().clean_roots,
        vec![root_a, root_b]
    );
    assert_eq!(storage::root_id(&cfg.cache_dir).unwrap(), id);
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_cargoc"))
        .arg("--cargoc-config")
        .arg(&path)
        .args(["config", "--clear-clean-roots"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(Config::load(&path).unwrap().clean_roots.is_empty());
}

#[cfg(unix)]
#[test]
fn binding_cli_rejects_fake_mount_without_saving_then_allows_explicit_unbinding() {
    let temp = tempfile::tempdir_in(fs::canonicalize(std::env::temp_dir()).unwrap()).unwrap();
    let mut cfg = Config {
        cache_dir: temp.path().join("cache"),
        ..Config::default()
    };
    Store::open(cfg.clone(), true).unwrap();
    let path = temp.path().join("config.toml");
    cfg.save(&path).unwrap();
    let before = fs::read(&path).unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_cargoc"))
        .arg("--cargoc-config")
        .arg(&path)
        .args(["config", "--require-mount"])
        .arg(temp.path())
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(fs::read(&path).unwrap(), before);
    cfg.require_mount = Some(temp.path().to_path_buf());
    cfg.required_root_id = Some(storage::root_id(&cfg.cache_dir).unwrap());
    cfg.save(&path).unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_cargoc"))
        .arg("--cargoc-config")
        .arg(&path)
        .arg("config")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "Configuration must remain readable offline"
    );
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_cargoc"))
        .arg("--cargoc-config")
        .arg(&path)
        .args(["config", "--clear-require-mount"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let saved = Config::load(&path).unwrap();
    assert!(saved.require_mount.is_none());
    assert!(saved.required_root_id.is_none());
}
