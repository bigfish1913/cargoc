use cargoc::config::{Config, migrate_legacy_config};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};
use tempfile::TempDir;

fn temp() -> TempDir {
    tempfile::tempdir_in(fs::canonicalize(std::env::temp_dir()).unwrap()).unwrap()
}

fn config(root: &Path, threshold: f64) -> Config {
    Config {
        cache_dir: root.join("cache"),
        max_cache_gib: threshold,
        min_free_gib: 0.0,
        abort_free_gib: 0.0,
        ..Config::default()
    }
}

fn command(root: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_cargoc"));
    cmd.arg("config")
        .current_dir(root)
        .env("HOME", root.join("home"))
        .env("USERPROFILE", root.join("home"))
        .env("APPDATA", root.join("appdata"))
        .env("XDG_CONFIG_HOME", root.join("xdg"))
        .env_remove("CARGO_HOME")
        .env_remove("CARGOC_CONFIG");
    cmd
}

fn legacy(root: &Path) -> PathBuf {
    root.join(if cfg!(windows) { "appdata" } else { "xdg" })
        .join("cargoc/config.toml")
}

fn assert_path(mut cmd: Command, expected: &Path) {
    let output = cmd.output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let output = String::from_utf8(output.stdout).unwrap();
    let printed = output.lines().next().unwrap().strip_prefix("# ").unwrap();
    assert_eq!(
        cargoc::util::identity_path(Path::new(printed)).unwrap(),
        cargoc::util::identity_path(expected).unwrap(),
        "{output}"
    );
}

#[test]
fn default_config_lives_beside_cargo_config() {
    let temp = temp();
    let destination = temp.path().join("home/.cargo/cargoc.toml");
    config(temp.path(), 100.0).save(&destination).unwrap();
    let cargo_config = destination.with_file_name("config.toml");
    fs::write(&cargo_config, "# Original Cargo configuration\n").unwrap();
    assert_path(command(temp.path()), &destination);
    assert_eq!(
        fs::read_to_string(cargo_config).unwrap(),
        "# Original Cargo configuration\n"
    );
}

#[test]
fn cargo_home_is_respected_including_relative_paths() {
    let temp = temp();
    let home = temp.path().join("custom-cargo-home");
    let destination = home.join("cargoc.toml");
    config(temp.path(), 100.0).save(&destination).unwrap();
    let mut cmd = command(temp.path());
    cmd.env("CARGO_HOME", &home);
    assert_path(cmd, &destination);
    let mut cmd = command(temp.path());
    cmd.env("CARGO_HOME", "custom-cargo-home");
    assert_path(cmd, &destination);
}

#[test]
fn explicit_config_overrides_cargo_home_and_does_not_migrate() {
    let temp = temp();
    let environment = temp.path().join("environment.toml");
    let explicit = temp.path().join("explicit.toml");
    config(temp.path(), 11.0).save(&environment).unwrap();
    config(temp.path(), 22.0).save(&explicit).unwrap();
    config(temp.path(), 100.0)
        .save(&legacy(temp.path()))
        .unwrap();
    let mut cmd = command(temp.path());
    cmd.env("CARGOC_CONFIG", &environment)
        .env("CARGO_HOME", temp.path().join("custom"));
    assert_path(cmd, &environment);
    let mut cmd = command(temp.path());
    cmd.env("CARGOC_CONFIG", &environment)
        .args(["--cargoc-config"])
        .arg(&explicit);
    assert_path(cmd, &explicit);
    assert!(!temp.path().join("home/.cargo/cargoc.toml").exists());
    assert!(!temp.path().join("custom/cargoc.toml").exists());
}

#[test]
fn legacy_config_migrates_without_changing_original_or_cargo_settings() {
    let temp = temp();
    let old = legacy(temp.path());
    config(temp.path(), 100.0).save(&old).unwrap();
    let original = fs::read(&old).unwrap();
    let destination = temp.path().join("home/.cargo/cargoc.toml");
    fs::create_dir_all(destination.parent().unwrap()).unwrap();
    let cargo_config = destination.with_file_name("config.toml");
    fs::write(&cargo_config, "# Do not modify\n").unwrap();
    assert_path(command(temp.path()), &destination);
    assert_eq!(Config::load(&destination).unwrap().max_cache_gib, 100.0);
    assert_eq!(fs::read(old).unwrap(), original);
    assert_eq!(
        fs::read_to_string(cargo_config).unwrap(),
        "# Do not modify\n"
    );
}

#[test]
fn migration_never_overwrites_new_config_and_rejects_invalid_legacy_data() {
    let temp = temp();
    let old = temp.path().join("old.toml");
    let new = temp.path().join("new.toml");
    config(temp.path(), 100.0).save(&old).unwrap();
    config(temp.path(), 42.0).save(&new).unwrap();
    assert!(!migrate_legacy_config(&new, &old).unwrap());
    assert_eq!(Config::load(&new).unwrap().max_cache_gib, 42.0);
    let missing = temp.path().join("missing.toml");
    fs::write(&old, "not valid toml").unwrap();
    assert!(migrate_legacy_config(&missing, &old).is_err());
    assert!(!missing.exists());
}
