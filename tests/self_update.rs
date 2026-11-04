//! Offline updater tests use disposable copies, never the installed cargoc.
use cargoc::{store::Lease, util::identity_path};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{fs, path::PathBuf, process::Command};
use tempfile::TempDir;

struct Fixture {
    home: TempDir,
    executable: PathBuf,
    directory: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let home = tempfile::tempdir_in(fs::canonicalize(std::env::temp_dir()).unwrap()).unwrap();
        let executable = home
            .path()
            .join(format!("cargoc{}", std::env::consts::EXE_SUFFIX));
        fs::copy(env!("CARGO_BIN_EXE_cargoc"), &executable).unwrap();
        let key = format!(
            "{:x}",
            Sha256::digest(identity_path(&executable).unwrap().as_bytes())
        );
        let directory = home.path().join(".cargoc/rollback").join(key);
        fs::create_dir_all(&directory).unwrap();
        Self {
            home,
            executable,
            directory,
        }
    }
    fn command(&self) -> Command {
        let mut command = Command::new(&self.executable);
        command
            .env("HOME", self.home.path())
            .env("USERPROFILE", self.home.path())
            // Deliberately invalid configuration must not prevent self-update.
            .env(
                "CARGOC_CONFIG",
                self.home.path().join("missing-config.toml"),
            );
        command
    }
    fn seed(&self, corrupt: bool) -> Vec<u8> {
        let mut bytes = fs::read(&self.executable).unwrap();
        // Do not alter Mach-O's ad-hoc code signature on Apple Silicon.
        if !cfg!(target_os = "macos") {
            bytes.extend_from_slice(b"\nrollback-fixture\n");
        }
        let file = format!("cargoc-fixture{}", std::env::consts::EXE_SUFFIX);
        let path = self.directory.join(&file);
        fs::write(&path, &bytes).unwrap();
        fs::set_permissions(&path, fs::metadata(&self.executable).unwrap().permissions()).unwrap();
        fs::write(self.directory.join("latest.json"), serde_json::to_vec(&json!({
            "file": file, "sha256": if corrupt { "bad".to_owned() } else { format!("{:x}", Sha256::digest(&bytes)) },
            "version": env!("CARGO_PKG_VERSION")
        })).unwrap()).unwrap();
        bytes
    }
}

#[test]
fn rollback_replaces_a_disposable_binary_and_keeps_the_original_backup() {
    let fixture = Fixture::new();
    let original = fs::read(&fixture.executable).unwrap();
    let expected = fixture.seed(false);
    let output = fixture
        .command()
        .args(["self-update", "--rollback"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(fs::read(&fixture.executable).unwrap(), expected);
    let metadata: Value =
        serde_json::from_slice(&fs::read(fixture.directory.join("latest.json")).unwrap()).unwrap();
    assert_eq!(
        fs::read(fixture.directory.join(metadata["file"].as_str().unwrap())).unwrap(),
        original
    );
    let output = fixture.command().arg("--version").output().unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap().trim(),
        format!("cargoc {}", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn corrupt_backup_cannot_change_the_executable() {
    let fixture = Fixture::new();
    let original = fs::read(&fixture.executable).unwrap();
    fixture.seed(true);
    let output = fixture
        .command()
        .args(["self-update", "--rollback"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("SHA-256"));
    assert_eq!(fs::read(&fixture.executable).unwrap(), original);
}

#[test]
fn concurrent_updaters_are_rejected_before_rollback() {
    let fixture = Fixture::new();
    fixture.seed(false);
    let _lock = Lease::acquire(&fixture.directory.join("update.lock"), true)
        .unwrap()
        .unwrap();
    let output = fixture
        .command()
        .args(["self-update", "--rollback"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Another cargoc update"));
}

#[test]
fn contradictory_update_flags_fail_without_network_or_file_changes() {
    let fixture = Fixture::new();
    let original = fs::read(&fixture.executable).unwrap();
    for flags in [["--check", "--rollback"], ["--force", "--rollback"]] {
        assert!(
            !fixture
                .command()
                .arg("self-update")
                .args(flags)
                .output()
                .unwrap()
                .status
                .success()
        );
    }
    assert_eq!(fs::read(&fixture.executable).unwrap(), original);
    assert!(!fixture.directory.join("update.lock").exists());
}

#[test]
fn wrong_version_or_escaping_backup_metadata_is_rejected() {
    let fixture = Fixture::new();
    let original = fs::read(&fixture.executable).unwrap();
    fixture.seed(false);
    let manifest = fixture.directory.join("latest.json");
    let mut metadata: Value = serde_json::from_slice(&fs::read(&manifest).unwrap()).unwrap();
    metadata["version"] = "999.0.0".into();
    fs::write(&manifest, serde_json::to_vec(&metadata).unwrap()).unwrap();
    let output = fixture
        .command()
        .args(["self-update", "--rollback"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("version mismatch"));
    metadata["file"] = "cargoc-../escape".into();
    fs::write(&manifest, serde_json::to_vec(&metadata).unwrap()).unwrap();
    let output = fixture
        .command()
        .args(["self-update", "--rollback"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Invalid rollback metadata"));
    assert_eq!(fs::read(&fixture.executable).unwrap(), original);
}
