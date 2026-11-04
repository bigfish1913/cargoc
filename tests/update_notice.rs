use std::{fs, process::Command};

fn temp() -> tempfile::TempDir {
    tempfile::tempdir_in(fs::canonicalize(std::env::temp_dir()).unwrap()).unwrap()
}

#[test]
fn disabled_or_offline_workers_do_not_touch_configuration_or_state() {
    for (name, value) in [
        ("CARGOC_DISABLE_UPDATE_CHECK", "1"),
        ("CARGO_NET_OFFLINE", "true"),
        ("CI", "true"),
    ] {
        let home = temp();
        let output = Command::new(env!("CARGO_BIN_EXE_cargoc"))
            .args(["__update-check", "github"])
            .env_remove("CI")
            .env_remove("CARGO_NET_OFFLINE")
            .env_remove("CARGOC_DISABLE_UPDATE_CHECK")
            .env(name, value)
            .env("HOME", home.path())
            .env("USERPROFILE", home.path())
            .env("CARGOC_CONFIG", home.path().join("invalid-config.toml"))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stdout.is_empty());
        assert!(output.stderr.is_empty());
        assert!(!home.path().join(".cargoc").exists());
    }
}

#[test]
fn version_and_help_have_no_helper_or_notification_output() {
    for arg in ["--version", "--help"] {
        let output = Command::new(env!("CARGO_BIN_EXE_cargoc"))
            .arg(arg)
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(output.stderr.is_empty());
        assert!(!String::from_utf8_lossy(&output.stdout).contains("__update-check"));
    }
}

#[test]
fn package_managed_installations_refuse_direct_replacement_even_with_force() {
    for (path, command) in [
        ("Cellar/cargoc/0.1.2/bin", "brew upgrade"),
        ("scoop/apps/cargoc/current", "scoop update cargoc"),
    ] {
        let home = temp();
        let dir = home.path().join(path);
        fs::create_dir_all(&dir).unwrap();
        let executable = dir.join(format!("cargoc{}", std::env::consts::EXE_SUFFIX));
        fs::copy(env!("CARGO_BIN_EXE_cargoc"), &executable).unwrap();
        let before = fs::read(&executable).unwrap();
        let output = Command::new(&executable)
            .args(["self-update", "--force"])
            .env("HOME", home.path())
            .env("USERPROFILE", home.path())
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains(command));
        assert_eq!(fs::read(&executable).unwrap(), before);
        assert!(!home.path().join(".cargoc").exists());
    }
}
