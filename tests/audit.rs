use cargoc::{
    audit::{self, LockState},
    config::Config,
    store::Lease,
};
use std::{fs, path::PathBuf, process::Command};
use tempfile::TempDir;

fn temp() -> TempDir {
    tempfile::tempdir_in(fs::canonicalize(std::env::temp_dir()).unwrap()).unwrap()
}

fn command(dir: &TempDir) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_cargoc"));
    command
        .env("HOME", dir.path())
        .env("USERPROFILE", dir.path())
        .env("CARGO_HOME", dir.path().join("cargo-home"))
        .env("APPDATA", dir.path().join("legacy"))
        .env("XDG_CONFIG_HOME", dir.path().join("legacy"))
        .env("CARGOC_DISABLE_UPDATE_CHECK", "1")
        .env_remove("CARGOC_CONFIG");
    command
}

#[test]
fn inventory_is_read_only_and_available_locks_are_not_deletion_permission() {
    let dir = temp();
    let external = dir.path().join("external");
    let profile = external.join("00/123456789abcde/debug");
    fs::create_dir_all(profile.join("deps")).unwrap();
    let source = profile.join("notes.txt");
    fs::write(&source, "never adopt me").unwrap();
    fs::write(profile.join("deps/intermediate"), [42; 1024]).unwrap();
    let lock = profile.join(".cargo-lock");
    fs::write(&lock, b"").unwrap();
    let modified = fs::metadata(&source).unwrap().modified().unwrap();
    let report = audit::inspect(&external, &dir.path().join("managed")).unwrap();
    assert!(report.read_only);
    assert_eq!(report.files, 3);
    assert_eq!(report.bytes, 1024 + "never adopt me".len() as u64);
    assert_eq!(report.locks.len(), 1);
    assert_eq!(report.locks[0].state, LockState::Available);
    assert!(report.warnings.iter().any(|w| w.contains("do not prove")));
    assert_eq!(fs::read_to_string(&source).unwrap(), "never adopt me");
    assert_eq!(fs::metadata(&source).unwrap().modified().unwrap(), modified);
    assert!(!external.join(".cargoc-root.json").exists());
    assert!(!dir.path().join("managed").exists());
    let held = Lease::acquire(&lock, false).unwrap().unwrap();
    let report = audit::inspect(&external, &dir.path().join("managed")).unwrap();
    assert_eq!(report.locks[0].state, LockState::Busy);
    assert!(Lease::acquire(&lock, false).unwrap().is_none());
    drop(held);
    assert!(Lease::acquire(&lock, false).unwrap().is_some());
}

#[test]
fn cli_does_not_need_configuration_or_migrate_legacy_settings() {
    let dir = temp();
    let external = dir.path().join("external cache 工具");
    fs::create_dir_all(&external).unwrap();
    fs::write(external.join("cache.bin"), [1; 123]).unwrap();
    let legacy = dir.path().join("legacy/cargoc/config.toml");
    let config = Config {
        cache_dir: dir.path().join("managed"),
        min_free_gib: 0.0,
        abort_free_gib: 0.0,
        ..Config::default()
    };
    config.save(&legacy).unwrap();
    let output = command(&dir)
        .arg("audit")
        .arg("--path")
        .arg(&external)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["read_only"], true);
    assert_eq!(report["bytes"], 123);
    assert_eq!(report["files"], 1);
    assert!(!dir.path().join("cargo-home").exists());
    assert!(!config.cache_dir.exists());
    assert!(legacy.is_file());
    assert!(!external.join(".cargo-lock").exists());
    // There is intentionally no external deletion/adoption switch.
    let rejected = command(&dir)
        .args(["audit", "--path"])
        .arg(&external)
        .arg("--delete")
        .output()
        .unwrap();
    assert!(!rejected.status.success());
    assert!(external.join("cache.bin").is_file());
}

#[test]
fn managed_overlap_markers_missing_directories_and_files_are_rejected() {
    let dir = temp();
    let managed = dir.path().join("managed");
    fs::create_dir_all(managed.join("child")).unwrap();
    for path in [
        dir.path(),
        managed.as_path(),
        managed.join("child").as_path(),
    ] {
        assert!(audit::inspect(path, &managed).is_err());
    }
    let foreign = dir.path().join("another-owned-root");
    fs::create_dir_all(&foreign).unwrap();
    fs::write(foreign.join(".cargoc-root.json"), "{}").unwrap();
    assert!(audit::inspect(&foreign, &managed).is_err());
    assert!(foreign.join(".cargoc-root.json").is_file());
    assert!(audit::inspect(&dir.path().join("missing"), &managed).is_err());
    let file = dir.path().join("file");
    fs::write(&file, "keep").unwrap();
    assert!(audit::inspect(&file, &managed).is_err());
}

#[test]
fn cli_respects_explicit_managed_root_without_initializing_it() {
    let dir = temp();
    let config_path = dir.path().join("custom.toml");
    let root = dir.path().join("managed");
    Config {
        cache_dir: root.clone(),
        ..Config::default()
    }
    .save(&config_path)
    .unwrap();
    fs::create_dir_all(&root).unwrap();
    let output = command(&dir)
        .arg("--cargoc-config")
        .arg(&config_path)
        .args(["audit", "--path"])
        .arg(&root)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("overlaps"));
    assert_eq!(fs::read_dir(&root).unwrap().count(), 0);
}

#[test]
fn empty_external_directory_is_not_assumed_idle() {
    let dir = temp();
    let external = dir.path().join("external");
    fs::create_dir_all(&external).unwrap();
    let report = audit::inspect(&external, &dir.path().join("managed")).unwrap();
    assert_eq!(report.bytes, 0);
    assert_eq!(report.files, 0);
    assert_eq!(report.directories, 1);
    assert!(
        report
            .warnings
            .iter()
            .any(|w| w.contains("no activity assessment"))
    );
    assert_eq!(fs::read_dir(external).unwrap().count(), 0);
}

#[test]
fn links_in_external_cache_are_never_followed() {
    let dir = temp();
    let external = dir.path().join("external");
    let source = dir.path().join("source");
    fs::create_dir_all(&external).unwrap();
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("important"), "keep source").unwrap();
    let link: PathBuf = external.join("link");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&source, &link).unwrap();
    #[cfg(windows)]
    {
        let output = Command::new("cmd")
            .args(["/c", "mklink", "/J"])
            .arg(&link)
            .arg(&source)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    assert!(audit::inspect(&external, &dir.path().join("managed")).is_err());
    assert!(audit::inspect(&link, &dir.path().join("managed")).is_err());
    assert_eq!(
        fs::read_to_string(source.join("important")).unwrap(),
        "keep source"
    );
}

#[cfg(windows)]
#[test]
fn readonly_lock_is_reported_unavailable_without_changing_attributes() {
    let dir = temp();
    let external = dir.path().join("external");
    fs::create_dir_all(&external).unwrap();
    let lock = external.join(".cargo-lock");
    fs::write(&lock, "keep").unwrap();
    let original = fs::metadata(&lock).unwrap().permissions();
    let mut readonly = original.clone();
    readonly.set_readonly(true);
    fs::set_permissions(&lock, readonly).unwrap();
    let report = audit::inspect(&external, &dir.path().join("managed")).unwrap();
    assert_eq!(report.locks[0].state, LockState::Unavailable);
    assert!(fs::metadata(&lock).unwrap().permissions().readonly());
    assert_eq!(fs::read_to_string(&lock).unwrap(), "keep");
    fs::set_permissions(&lock, original).unwrap();
}
