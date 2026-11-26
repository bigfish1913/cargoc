use cargoc::{
    config::Config,
    global_clean::{self, Options},
    store::{Lease, Store},
};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};
use tempfile::TempDir;

struct Fixture {
    _temp: TempDir,
    root: PathBuf,
    config: Config,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir_in(fs::canonicalize(std::env::temp_dir()).unwrap()).unwrap();
        let root = temp.path().join("projects with spaces");
        fs::create_dir(&root).unwrap();
        let config = Config {
            cache_dir: temp.path().join("managed"),
            cargo: std::env::var_os("CARGO")
                .unwrap_or_else(|| "cargo".into())
                .into(),
            clean_roots: vec![root.clone()],
            min_free_gib: 0.0,
            abort_free_gib: 0.0,
            ..Config::default()
        };
        Store::open(config.clone(), true).unwrap();
        Self {
            _temp: temp,
            root,
            config,
        }
    }
    fn project(&self, name: &str) -> PathBuf {
        let dir = self.root.join(name);
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(
            dir.join("Cargo.toml"),
            format!(
                "[package]\nname = '{name}'\nversion = '0.1.0'\nedition = '2021'\n[workspace]\n"
            ),
        )
        .unwrap();
        fs::write(dir.join("src/main.rs"), "fn main() { println!(\"ok\"); }").unwrap();
        fs::write(
            dir.join("Cargo.lock"),
            format!("version = 4\n[[package]]\nname = '{name}'\nversion = '0.1.0'\n"),
        )
        .unwrap();
        fs::create_dir_all(dir.join("target/debug")).unwrap();
        fs::write(
            dir.join("target/CACHEDIR.TAG"),
            b"Signature: 8a477f597d28d172789f06886806bc55\n",
        )
        .unwrap();
        fs::write(dir.join("target/debug/artifact"), [42; 1024]).unwrap();
        dir
    }
    fn options(&self, dry_run: bool) -> Options {
        Options {
            include_targets: true,
            dry_run,
            yes: !dry_run,
            timeout_seconds: 10,
            ..Options::default()
        }
    }
    fn cli(&self) -> Command {
        let path = self._temp.path().join("config.toml");
        self.config.save(&path).unwrap();
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_cargoc"));
        cmd.arg("--cargoc-config")
            .arg(path)
            .env("CARGOC_DISABLE_UPDATE_CHECK", "1");
        cmd
    }
}
fn success(output: &Output) {
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
fn snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut files = BTreeMap::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else {
                files.insert(
                    path.strip_prefix(root).unwrap().into(),
                    fs::read(path).unwrap(),
                );
            }
        }
    }
    files
}

#[test]
fn dry_run_discovers_multiple_projects_and_writes_nothing() {
    let f = Fixture::new();
    f.project("first");
    f.project("second");
    let before = snapshot(f._temp.path());
    let report = global_clean::execute(&f.config, &f.options(true)).unwrap();
    assert!(!report.incomplete, "{:?}", report.skipped);
    assert_eq!(report.targets.len(), 2);
    for entry in &report.targets {
        assert!(entry.before_bytes >= 1024);
        assert_eq!(entry.status, "planned");
        assert!(entry.deletes_final_artifacts);
    }
    assert_eq!(snapshot(f._temp.path()), before);
}

#[test]
fn authorization_and_search_roots_are_required_before_any_deletion() {
    let mut f = Fixture::new();
    let project = f.project("keep");
    let mut options = f.options(false);
    options.yes = false;
    assert!(global_clean::execute(&f.config, &options).is_err());
    options.yes = true;
    f.config.clean_roots.clear();
    assert!(global_clean::execute(&f.config, &options).is_err());
    options.roots = vec![cargoc::config::home()];
    assert!(global_clean::execute(&f.config, &options).is_err());
    options.roots = vec![f.config.cache_dir.clone()];
    assert!(global_clean::execute(&f.config, &options).is_err());
    assert!(project.join("target/debug/artifact").is_file());
}

#[test]
fn native_clean_removes_two_targets_but_preserves_sources_and_external_build_dir() {
    let f = Fixture::new();
    let first = f.project("realone");
    let second = f.project("realtwo");
    // Build one actual executable using native Cargo, not the user's managed cache.
    let target = first.join("target");
    let build_config = format!(
        "build.build-dir={}",
        toml::Value::String(target.to_str().unwrap().into())
    );
    success(
        &Command::new(&f.config.cargo)
            .current_dir(&first)
            .args(["--config", &build_config, "build", "--offline"])
            .env("CARGO_BUILD_BUILD_DIR", &target)
            .env("CARGO_TARGET_DIR", &target)
            .env("CARGO_BUILD_TARGET_DIR", &target)
            .env_remove("RUSTC_WRAPPER")
            .env_remove("RUSTC_WORKSPACE_WRAPPER")
            .output()
            .unwrap(),
    );
    let binary = target
        .join("debug")
        .join(format!("realone{}", std::env::consts::EXE_SUFFIX));
    success(&Command::new(&binary).output().unwrap());
    // Legacy global/project build-dir must not expand native clean's deletion scope.
    let external = f._temp.path().join("legacy-external");
    fs::create_dir(&external).unwrap();
    fs::write(external.join("sentinel"), "preserve").unwrap();
    fs::create_dir(first.join(".cargo")).unwrap();
    fs::write(
        first.join(".cargo/config.toml"),
        format!(
            "[build]\nbuild-dir = {}\n",
            toml::Value::String(external.to_str().unwrap().into())
        ),
    )
    .unwrap();
    let first_source = fs::read(first.join("src/main.rs")).unwrap();
    let lock = fs::read(first.join("Cargo.lock")).unwrap();
    // Existing managed-only clean must still preserve the executable.
    success(&f.cli().arg("clean").output().unwrap());
    assert!(binary.is_file());
    let report = global_clean::execute(&f.config, &f.options(false)).unwrap();
    assert!(
        !report.incomplete,
        "{}",
        serde_json::to_string(&report).unwrap()
    );
    assert_eq!(report.targets.len(), 2);
    assert!(
        report
            .targets
            .iter()
            .all(|t| t.status == "cleaned" && t.removed_bytes > 0)
    );
    assert!(!binary.exists());
    assert!(!second.join("target/debug/artifact").exists());
    assert_eq!(fs::read(first.join("src/main.rs")).unwrap(), first_source);
    assert_eq!(fs::read(first.join("Cargo.lock")).unwrap(), lock);
    assert_eq!(
        fs::read_to_string(external.join("sentinel")).unwrap(),
        "preserve"
    );
}

#[test]
fn modern_native_locks_and_managed_leases_block_target_cleanup() {
    let f = Fixture::new();
    let first = f.project("busyone");
    let second = f.project("busytwo");
    let native_path = first.join("target/debug/.cargo-build-lock");
    let native = Lease::acquire(&native_path, true).unwrap().unwrap();
    let store = Store::open(f.config.clone(), false).unwrap();
    let key = Store::key(&second).unwrap();
    let managed = store.shared_lease(&key).unwrap().unwrap();
    for dry in [true, false] {
        let report = global_clean::execute(&f.config, &f.options(dry)).unwrap();
        assert!(report.incomplete);
        assert_eq!(report.targets.len(), 2);
        assert!(report.targets.iter().all(|t| t.status == "skipped"));
        assert!(first.join("target/debug/artifact").exists());
        assert!(second.join("target/debug/artifact").exists());
    }
    drop(native);
    drop(managed);
    assert!(Lease::acquire(&native_path, false).unwrap().is_some());
}

#[test]
fn unknown_root_files_and_source_manifests_are_not_adopted() {
    let f = Fixture::new();
    let first = f.project("notes");
    let second = f.project("source");
    fs::write(first.join("target/notes.txt"), "keep notes").unwrap();
    fs::write(second.join("target/debug/Cargo.toml"), "keep source").unwrap();
    let before = snapshot(&f.root);
    let report = global_clean::execute(&f.config, &f.options(false)).unwrap();
    assert!(report.incomplete);
    assert_eq!(report.targets.len(), 0);
    assert_eq!(report.skipped.len(), 2);
    assert_eq!(snapshot(&f.root), before);
}

#[cfg(unix)]
#[test]
fn symlinks_inside_target_or_as_target_are_never_followed() {
    let f = Fixture::new();
    let first = f.project("symlinkone");
    let second = f.project("symlinktwo");
    let external = f._temp.path().join("external");
    fs::create_dir(&external).unwrap();
    fs::write(external.join("precious"), "keep").unwrap();
    std::os::unix::fs::symlink(&external, first.join("target/debug/link")).unwrap();
    fs::remove_dir_all(second.join("target")).unwrap();
    std::os::unix::fs::symlink(&external, second.join("target")).unwrap();
    let report = global_clean::execute(&f.config, &f.options(false)).unwrap();
    assert!(report.incomplete);
    assert!(report.targets.is_empty());
    assert!(first.join("target/debug/artifact").exists());
    assert_eq!(
        fs::read_to_string(external.join("precious")).unwrap(),
        "keep"
    );
}

#[test]
fn disconnected_managed_volume_does_not_prevent_authorized_internal_target_cleanup() {
    let mut f = Fixture::new();
    let project = f.project("internal");
    let missing = f._temp.path().join("unmounted");
    f.config.cache_dir = missing.join("cache");
    f.config.require_mount = Some(missing.clone());
    f.config.required_root_id = Some("a".repeat(64));
    // Config is structurally unsupported on Windows; simulate a missing unbound cache there.
    if cfg!(windows) {
        f.config.require_mount = None;
        f.config.required_root_id = None;
    }
    let before = snapshot(&f.root);
    let preview = global_clean::execute(&f.config, &f.options(true)).unwrap();
    assert!(preview.managed_error.is_some());
    assert_eq!(preview.targets.len(), 1);
    assert_eq!(snapshot(&f.root), before);
    let report = global_clean::execute(&f.config, &f.options(false)).unwrap();
    assert!(report.incomplete);
    assert!(report.managed_error.is_some());
    assert_eq!(report.targets[0].status, "cleaned");
    assert!(!project.join("target/debug/artifact").exists());
    assert!(!missing.exists());
}

#[test]
fn nested_workspaces_are_found_and_member_targets_deduplicated() {
    let f = Fixture::new();
    let outer = f.project("outer");
    fs::write(
        outer.join("Cargo.toml"),
        "[workspace]\nmembers = ['member']\nexclude = ['nested']\nresolver = '2'\n",
    )
    .unwrap();
    fs::remove_file(outer.join("Cargo.lock")).unwrap();
    fs::write(
        outer.join("Cargo.lock"),
        "version = 4\n[[package]]\nname = 'member'\nversion = '0.1.0'\n",
    )
    .unwrap();
    let member = outer.join("member");
    fs::create_dir_all(member.join("src")).unwrap();
    fs::create_dir_all(member.join("target/debug")).unwrap();
    fs::write(
        member.join("Cargo.toml"),
        "[package]\nname='member'\nversion='0.1.0'\nedition='2021'\n",
    )
    .unwrap();
    fs::write(member.join("src/lib.rs"), "").unwrap();
    let nested = outer.join("nested");
    fs::create_dir_all(nested.join("src")).unwrap();
    fs::create_dir_all(nested.join("target/debug")).unwrap();
    fs::write(
        nested.join("target/CACHEDIR.TAG"),
        b"Signature: 8a477f597d28d172789f06886806bc55\n",
    )
    .unwrap();
    fs::write(
        nested.join("Cargo.toml"),
        "[package]\nname='nested'\nversion='0.1.0'\nedition='2021'\n[workspace]\n",
    )
    .unwrap();
    fs::write(nested.join("src/lib.rs"), "").unwrap();
    fs::write(
        nested.join("Cargo.lock"),
        "version=4\n[[package]]\nname='nested'\nversion='0.1.0'\n",
    )
    .unwrap();
    let mut options = f.options(true);
    options.roots = vec![f.root.clone(), outer.clone()];
    let report = global_clean::execute(&f.config, &options).unwrap();
    assert!(!report.incomplete, "{:?}", report.skipped);
    assert_eq!(report.targets.len(), 2);
    assert!(report.targets.iter().any(|t| t.workspace == outer));
    assert!(report.targets.iter().any(|t| t.workspace == nested));
}

#[test]
fn cli_flags_and_json_are_explicit_and_managed_only_remains_compatible() {
    let f = Fixture::new();
    let project = f.project("cli");
    let output = f
        .cli()
        .args(["clean", "--include-targets", "--yes"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let output = f
        .cli()
        .args(["clean", "--global", "--include-targets"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let output = f.cli().args(["clean", "--dry-run"]).output().unwrap();
    success(&output);
    let managed: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(managed.get("before_bytes").is_some());
    assert!(managed.get("targets").is_none());
    let output = f
        .cli()
        .args(["clean", "--global", "--include-targets", "--dry-run"])
        .output()
        .unwrap();
    success(&output);
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["targets"].as_array().unwrap().len(), 1);
    assert!(project.join("target/debug/artifact").is_file());
    let output = f
        .cli()
        .args(["clean", "--global", "--include-targets", "--yes"])
        .output()
        .unwrap();
    success(&output);
    assert!(!project.join("target/debug/artifact").exists());
}

#[test]
fn metadata_failure_is_reported_without_touching_target() {
    let f = Fixture::new();
    let project = f.project("broken");
    fs::write(project.join("Cargo.toml"), "not a manifest").unwrap();
    let before = snapshot(&f.root);
    let report = global_clean::execute(&f.config, &f.options(false)).unwrap();
    assert!(report.incomplete);
    assert_eq!(report.targets.len(), 0);
    assert_eq!(report.skipped.len(), 1);
    assert_eq!(snapshot(&f.root), before);
}

#[test]
fn untagged_targets_and_custom_output_directories_are_not_deleted() {
    let f = Fixture::new();
    let untagged = f.project("untagged");
    fs::remove_file(untagged.join("target/CACHEDIR.TAG")).unwrap();
    let custom = f.project("custom");
    fs::rename(custom.join("target"), custom.join("custom-output")).unwrap();
    fs::create_dir(custom.join(".cargo")).unwrap();
    fs::write(
        custom.join(".cargo/config.toml"),
        "[build]\ntarget-dir='custom-output'\n",
    )
    .unwrap();
    let before = snapshot(&f.root);
    let report = global_clean::execute(&f.config, &f.options(false)).unwrap();
    assert!(report.incomplete);
    assert!(report.targets.is_empty());
    assert_eq!(snapshot(&f.root), before);
}

#[test]
fn git_worktree_git_files_do_not_prevent_independent_project_discovery() {
    let f = Fixture::new();
    let first = f.project("worktreeone");
    let second = f.project("worktreetwo");
    fs::write(
        first.join(".git"),
        "gitdir: /unrelated/.git/worktrees/one\n",
    )
    .unwrap();
    fs::write(
        second.join(".git"),
        "gitdir: /unrelated/.git/worktrees/two\n",
    )
    .unwrap();
    let report = global_clean::execute(&f.config, &f.options(true)).unwrap();
    assert!(!report.incomplete, "{:?}", report.skipped);
    assert_eq!(report.targets.len(), 2);
}

#[test]
fn partial_global_failure_has_nonzero_cli_exit_and_keeps_other_projects_independent() {
    let f = Fixture::new();
    let first = f.project("valid");
    let second = f.project("invalid");
    fs::write(second.join("target/manual-notes.txt"), "keep").unwrap();
    let output = f
        .cli()
        .args(["clean", "--global", "--include-targets", "--yes"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["incomplete"], true);
    assert!(!first.join("target/debug/artifact").exists());
    assert!(second.join("target/debug/artifact").exists());
    assert!(second.join("target/manual-notes.txt").exists());
}

#[test]
fn cargo_template_braces_cannot_expand_an_authorized_target_into_another_path() {
    let f = Fixture::new();
    let project = f.project("literal");
    let renamed = f.root.join("literal-{workspace-path-hash}");
    fs::rename(project, &renamed).unwrap();
    let before = snapshot(&f.root);
    let report = global_clean::execute(&f.config, &f.options(false)).unwrap();
    assert!(report.incomplete);
    assert!(report.targets.is_empty());
    assert!(report.skipped.iter().any(|s| s.contains("template braces")));
    assert_eq!(snapshot(&f.root), before);
}

#[test]
fn missing_lockfile_is_not_created_by_dry_run() {
    let f = Fixture::new();
    let project = f.project("nolock");
    fs::remove_file(project.join("Cargo.lock")).unwrap();
    let before = snapshot(&f.root);
    let _report = global_clean::execute(&f.config, &f.options(true)).unwrap();
    assert!(!project.join("Cargo.lock").exists());
    assert_eq!(snapshot(&f.root), before);
}
