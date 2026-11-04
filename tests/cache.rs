use cargoc::{
    config::Config,
    runner::{os_args, validate_cargo_args},
    store::{GcOptions, Lease, Store},
    util::{GIB, now},
};
use std::{
    fs,
    path::{Path, PathBuf},
};
use tempfile::TempDir;

fn temp() -> TempDir {
    tempfile::tempdir_in(fs::canonicalize(std::env::temp_dir()).unwrap()).unwrap()
}

fn config(root: &Path) -> Config {
    Config {
        cache_dir: root.join("cache"),
        min_free_gib: 0.0,
        abort_free_gib: 0.0,
        max_age_days: 0.0,
        ..Config::default()
    }
}

fn fixture() -> (TempDir, Store) {
    let temp = temp();
    let store = Store::open(config(temp.path()), true).unwrap();
    (temp, store)
}

fn add(store: &Store, root: &Path, name: &str, bytes: usize, age: f64) -> (String, PathBuf) {
    let workspace = root.join(name);
    fs::create_dir_all(workspace.join("src")).unwrap();
    fs::write(workspace.join("Cargo.toml"), "source manifest").unwrap();
    let key = Store::key(&workspace).unwrap();
    let _lease = store.lease(&key).unwrap().unwrap();
    let (_, path) = store.touch(&workspace).unwrap();
    fs::create_dir_all(path.join("debug/deps")).unwrap();
    fs::write(path.join("debug/deps/cache.bin"), vec![42_u8; bytes]).unwrap();
    let record_path = store.root.join("state").join(format!("{key}.json"));
    let mut data: serde_json::Value =
        serde_json::from_slice(&fs::read(&record_path).unwrap()).unwrap();
    data["last_used"] = serde_json::json!(now() - age);
    fs::write(record_path, serde_json::to_vec(&data).unwrap()).unwrap();
    (key, path)
}

#[test]
fn rejects_unowned_nonempty_root() {
    let temp = temp();
    let cfg = config(temp.path());
    fs::create_dir_all(&cfg.cache_dir).unwrap();
    let sentinel = cfg.cache_dir.join("source.rs");
    fs::write(&sentinel, "precious").unwrap();
    assert!(Store::open(cfg, true).is_err());
    assert_eq!(fs::read_to_string(sentinel).unwrap(), "precious");
}

#[test]
fn ownership_marker_is_required_and_validated() {
    let temp = temp();
    assert!(Store::open(config(temp.path()), false).is_err());
    let cfg = config(temp.path());
    Store::open(cfg.clone(), true).unwrap();
    fs::write(cfg.cache_dir.join(".cargoc-root.json"), "{}").unwrap();
    assert!(Store::open(cfg, true).is_err());
}

#[test]
fn source_workspace_cannot_be_inside_cache_root() {
    let (_temp, store) = fixture();
    assert!(store.workspace(&store.root.join("src")).is_err());
    assert!(store.workspace(&store.root).is_err());
}

#[test]
fn workspace_paths_are_separate_and_deterministic() {
    let (temp, store) = fixture();
    let a = store.workspace(&temp.path().join("a")).unwrap();
    let b = store.workspace(&temp.path().join("b")).unwrap();
    assert_ne!(a, b);
    assert_eq!(a, store.workspace(&temp.path().join("a")).unwrap());
    assert!(a.1.starts_with(store.root.join("workspaces")));
}

#[test]
fn pressure_collects_lru_until_low_water_mark() {
    let temp = temp();
    let mut cfg = config(temp.path());
    cfg.max_cache_gib = 9000.0 / GIB;
    let store = Store::open(cfg, true).unwrap();
    let (a, pa) = add(&store, temp.path(), "oldest", 4096, 30.0);
    let (b, pb) = add(&store, temp.path(), "middle", 4096, 20.0);
    let (_, pc) = add(&store, temp.path(), "newest", 4096, 10.0);
    let report = store
        .collect_with_free(&GcOptions::default(), || Ok(u64::MAX))
        .unwrap();
    assert_eq!(report.removed, [a, b]);
    assert_eq!(report.removed_bytes, 8192);
    assert_eq!(report.after_bytes, 4096);
    assert!(!report.unresolved);
    assert!(!pa.join("debug/deps/cache.bin").exists());
    assert!(!pb.join("debug/deps/cache.bin").exists());
    assert!(pc.join("debug/deps/cache.bin").exists());
}

#[test]
fn stays_below_high_water_without_churning_to_low_water() {
    let temp = temp();
    let mut cfg = config(temp.path());
    cfg.max_cache_gib = 9000.0 / GIB;
    let store = Store::open(cfg, true).unwrap();
    add(&store, temp.path(), "one", 8192, 0.0);
    let report = store.collect(&GcOptions::default()).unwrap();
    assert_eq!(report.removed_bytes, 0);
}

#[test]
fn dry_run_reports_but_preserves_bytes() {
    let (temp, store) = fixture();
    let (_, path) = add(&store, temp.path(), "project", 4096, 0.0);
    let report = store
        .collect(&GcOptions {
            all_idle: true,
            dry_run: true,
            ..GcOptions::default()
        })
        .unwrap();
    assert_eq!(report.removed_bytes, 4096);
    assert_eq!(
        fs::metadata(path.join("debug/deps/cache.bin"))
            .unwrap()
            .len(),
        4096
    );
}

#[test]
fn wrapper_lease_blocks_collection() {
    let (temp, store) = fixture();
    let (key, path) = add(&store, temp.path(), "project", 4096, 0.0);
    let _lease = store.lease(&key).unwrap().unwrap();
    assert!(store.lease(&key).unwrap().is_none());
    let report = store
        .collect(&GcOptions {
            all_idle: true,
            ..GcOptions::default()
        })
        .unwrap();
    assert_eq!(report.removed_bytes, 0);
    assert_eq!(report.skipped.len(), 1);
    assert!(path.join("debug/deps/cache.bin").exists());
}

#[test]
fn explicit_protection_blocks_collection() {
    let (temp, store) = fixture();
    let (key, path) = add(&store, temp.path(), "project", 4096, 0.0);
    let report = store
        .collect(&GcOptions {
            all_idle: true,
            protected: [key].into(),
            ..GcOptions::default()
        })
        .unwrap();
    assert_eq!(report.removed_bytes, 0);
    assert!(path.join("debug/deps/cache.bin").exists());
}

#[test]
fn native_lock_blocks_collection_and_is_never_replaced() {
    let (temp, store) = fixture();
    let (_, path) = add(&store, temp.path(), "project", 4096, 0.0);
    let lock_path = path.join("debug/.cargo-lock");
    let lock = Lease::acquire(&lock_path, true).unwrap().unwrap();
    let options = GcOptions {
        all_idle: true,
        ..GcOptions::default()
    };
    assert_eq!(store.collect(&options).unwrap().removed_bytes, 0);
    assert!(path.join("debug/deps/cache.bin").exists());
    drop(lock);
    assert_eq!(store.collect(&options).unwrap().removed_bytes, 4096);
    assert!(lock_path.is_file());
    assert!(Lease::acquire(&lock_path, false).unwrap().is_some());
}

#[test]
fn manual_clean_preserves_sources_final_artifacts_and_unknown_content() {
    let (temp, store) = fixture();
    add(&store, temp.path(), "project", 4096, 0.0);
    let final_path = temp.path().join("project/target/debug/app.exe");
    fs::create_dir_all(final_path.parent().unwrap()).unwrap();
    fs::write(&final_path, "final artifact").unwrap();
    let unknown = store.root.join("workspaces/untracked/notes.md");
    fs::create_dir_all(unknown.parent().unwrap()).unwrap();
    fs::write(&unknown, "unknown content").unwrap();
    store
        .collect(&GcOptions {
            all_idle: true,
            ..GcOptions::default()
        })
        .unwrap();
    assert!(temp.path().join("project/Cargo.toml").is_file());
    assert_eq!(fs::read_to_string(final_path).unwrap(), "final artifact");
    assert_eq!(fs::read_to_string(unknown).unwrap(), "unknown content");
}

#[test]
fn gc_lock_serializes_collectors() {
    let (temp, store) = fixture();
    add(&store, temp.path(), "project", 4096, 0.0);
    let _gc = Lease::acquire(&store.root.join("locks/gc.lock"), true)
        .unwrap()
        .unwrap();
    assert!(
        store
            .collect(&GcOptions {
                all_idle: true,
                ..GcOptions::default()
            })
            .unwrap()
            .busy
    );
}

#[test]
fn age_collection_uses_last_wrapper_use() {
    let temp = temp();
    let mut cfg = config(temp.path());
    cfg.max_age_days = 7.0;
    let store = Store::open(cfg, true).unwrap();
    let (key, _) = add(&store, temp.path(), "old", 4096, 8.0 * 86400.0);
    let (_, recent) = add(&store, temp.path(), "recent", 4096, 0.0);
    assert_eq!(store.collect(&GcOptions::default()).unwrap().removed, [key]);
    assert!(recent.join("debug/deps/cache.bin").exists());
}

#[test]
fn low_free_space_is_a_trigger_and_not_logical_free_space() {
    let temp = temp();
    let mut cfg = config(temp.path());
    cfg.min_free_gib = 1.0;
    let store = Store::open(cfg, true).unwrap();
    add(&store, temp.path(), "one", 4096, 0.0);
    let report = store
        .collect_with_free(&GcOptions::default(), || Ok(0))
        .unwrap();
    assert_eq!(report.removed_bytes, 4096);
    assert_eq!(report.free_bytes, 0); // Not inferred from logical byte counts.
    assert!(report.unresolved);
}

#[test]
fn corrupt_record_aborts_before_deleting_anything() {
    let (temp, store) = fixture();
    let (key, path) = add(&store, temp.path(), "project", 4096, 0.0);
    fs::write(store.root.join("state").join(format!("{key}.json")), "{}").unwrap();
    assert!(
        store
            .collect(&GcOptions {
                all_idle: true,
                ..GcOptions::default()
            })
            .is_err()
    );
    assert!(path.join("debug/deps/cache.bin").exists());
}

#[test]
fn invalid_thresholds_and_unknown_settings_are_rejected() {
    let temp = temp();
    for invalid in [f64::NAN, f64::INFINITY, -1.0, 0.0] {
        let mut cfg = config(temp.path());
        cfg.max_cache_gib = invalid;
        assert!(cfg.validate().is_err());
    }
    let mut cfg = config(temp.path());
    cfg.poll_seconds = 0.01;
    assert!(cfg.validate().is_err());
    cfg.poll_seconds = 5.0;
    cfg.min_free_gib = 1.0;
    cfg.abort_free_gib = 2.0;
    assert!(cfg.validate().is_err());
    let file = temp.path().join("config.toml");
    fs::write(&file, "unknown_option = true\n").unwrap();
    assert!(Config::load(&file).is_err());
}

#[test]
fn config_roundtrip_and_atomic_update() {
    let temp = temp();
    let mut cfg = config(temp.path());
    let path = temp.path().join("config.toml");
    cfg.save(&path).unwrap();
    assert_eq!(Config::load(&path).unwrap().cache_dir, cfg.cache_dir);
    cfg.max_cache_gib = 25.0;
    cfg.save(&path).unwrap();
    assert_eq!(Config::load(&path).unwrap().max_cache_gib, 25.0);
}

#[test]
fn cargo_config_overrides_cannot_escape_managed_root() {
    for args in [
        vec!["build", "--config", "build.build-dir='elsewhere'"],
        vec!["--config=build={build-dir='elsewhere'}", "build"],
        vec!["build", "--build-dir=elsewhere"],
        vec!["-C", "elsewhere", "build"],
    ] {
        assert!(validate_cargo_args(&os_args(&args)).is_err(), "{args:?}");
    }
    assert!(validate_cargo_args(&os_args(&["build", "--config", "net.offline=true"])).is_ok());
    assert!(
        validate_cargo_args(&os_args(&[
            "run",
            "--",
            "--config",
            "build.build-dir='program-option'"
        ]))
        .is_ok()
    );
    let temp = temp();
    let file = temp.path().join("override.toml");
    fs::write(&file, "[build]\nbuild-dir='elsewhere'\n").unwrap();
    assert!(validate_cargo_args(&os_args(&["build", "--config", file.to_str().unwrap()])).is_err());
}

#[cfg(unix)]
#[test]
fn symlink_in_managed_tree_or_root_is_never_followed() {
    use std::os::unix::fs::symlink;
    let (temp, store) = fixture();
    let (_, path) = add(&store, temp.path(), "project", 4096, 0.0);
    let outside = temp.path().join("outside");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("precious"), "safe").unwrap();
    symlink(&outside, path.join("debug/link")).unwrap();
    assert!(
        store
            .collect(&GcOptions {
                all_idle: true,
                ..GcOptions::default()
            })
            .is_err()
    );
    assert_eq!(
        fs::read_to_string(outside.join("precious")).unwrap(),
        "safe"
    );
    let linked = temp.path().join("linked-root");
    symlink(&store.root, &linked).unwrap();
    let mut cfg = config(temp.path());
    cfg.cache_dir = linked;
    assert!(Store::open(cfg, false).is_err());
}

#[cfg(windows)]
#[test]
fn windows_junction_is_never_followed() {
    let (temp, store) = fixture();
    let (_, path) = add(&store, temp.path(), "project", 4096, 0.0);
    let outside = temp.path().join("outside");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("precious"), "safe").unwrap();
    let junction = path.join("debug/link");
    let status = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(&junction)
        .arg(&outside)
        .output()
        .unwrap();
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    assert!(
        store
            .collect(&GcOptions {
                all_idle: true,
                ..GcOptions::default()
            })
            .is_err()
    );
    assert_eq!(
        fs::read_to_string(outside.join("precious")).unwrap(),
        "safe"
    );
    fs::remove_dir(junction).unwrap();
}

#[cfg(windows)]
#[test]
fn windows_identity_normalizes_case_verbatim_prefixes_and_source_boundaries() {
    let (_temp, store) = fixture();
    let path = store.root.join("some-source");
    let upper = PathBuf::from(path.to_str().unwrap().to_uppercase());
    assert!(store.workspace(&upper).is_err());
    assert_eq!(Store::key(&path).unwrap(), Store::key(&upper).unwrap());
    let normal = PathBuf::from(r"C:\Users\example\workspace");
    let verbatim = PathBuf::from(r"\\?\C:\Users\example\workspace");
    assert_eq!(Store::key(&normal).unwrap(), Store::key(&verbatim).unwrap());
}
