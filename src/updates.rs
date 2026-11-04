//! Cached, best-effort update notices. The foreground path performs no HTTP.
use crate::{
    Result, fail,
    installation::InstallSource,
    store::Lease,
    util::{atomic_write, now, read_bounded},
};
use semver::Version;
use serde::{Deserialize, Serialize};
use std::{
    ffi::OsString,
    io::{IsTerminal, Read},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

const INTERVAL: f64 = 24.0 * 3600.0;
const RETRY: f64 = 3600.0;
const MAX_AGE: f64 = 7.0 * 24.0 * 3600.0;

#[derive(Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Cache {
    attempted_at: f64,
    checked_at: f64,
    latest: Option<String>,
}

fn fresh(timestamp: f64, time: f64, ttl: f64) -> bool {
    timestamp > 0.0 && (0.0..=ttl).contains(&(time - timestamp))
}
fn read_cache(path: &Path) -> Cache {
    read_bounded(path, 16 * 1024)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}
fn cache_path(source: InstallSource) -> PathBuf {
    crate::config::home()
        .join(".cargoc")
        .join("updates")
        .join(source.key())
        .join("check.json")
}
fn disabled() -> bool {
    std::env::var_os("CARGOC_DISABLE_UPDATE_CHECK").is_some()
        || std::env::var("CARGO_NET_OFFLINE")
            .is_ok_and(|value| value == "1" || value.eq_ignore_ascii_case("true"))
        || std::env::var_os("CI").is_some()
}
fn eligible(args: &[OsString]) -> bool {
    let args: Vec<_> = args
        .iter()
        .take_while(|arg| *arg != "--")
        .map(|arg| arg.to_string_lossy())
        .collect();
    if args.iter().any(|arg| {
        matches!(
            arg.as_ref(),
            "--offline"
                | "--frozen"
                | "--help"
                | "-h"
                | "--version"
                | "-V"
                | "--quiet"
                | "-q"
                | "--message-format"
        ) || arg.starts_with("--message-format=")
            || arg
                .split_whitespace()
                .collect::<String>()
                .contains("net.offline=true")
    }) {
        return false;
    }
    let mut words = args.iter();
    let first = words.next();
    let command = if first.is_some_and(|s| s.starts_with('+')) {
        words.next()
    } else {
        first
    };
    command.is_some_and(|arg| {
        matches!(
            arg.as_ref(),
            "build"
                | "check"
                | "test"
                | "run"
                | "bench"
                | "doc"
                | "clippy"
                | "rustc"
                | "rustdoc"
                | "fix"
        )
    })
}
fn due(cache: &Cache, time: f64) -> bool {
    !fresh(cache.checked_at, time, INTERVAL) && !fresh(cache.attempted_at, time, RETRY)
}
fn notice(cache: &Cache, time: f64, current: &str, source: InstallSource) -> Option<String> {
    if !fresh(cache.checked_at, time, MAX_AGE) {
        return None;
    }
    let latest = cache.latest.as_deref()?;
    let latest_version = Version::parse(latest).ok()?;
    if latest.len() > 128
        || !latest_version.pre.is_empty()
        || !latest_version
            .cmp_precedence(&Version::parse(current).ok()?)
            .is_gt()
    {
        return None;
    }
    Some(format!(
        "cargoc: update available {current} -> {latest} ({}); run: {}",
        source.key(),
        source.update_command()
    ))
}

/// Launch at most one short-lived checker per source/interval. Only eligible
/// interactive builds call this; the child's output and exit code are discarded.
pub fn start(args: &[OsString]) -> bool {
    if disabled() || !std::io::stderr().is_terminal() || !eligible(args) {
        return false;
    }
    let source = crate::installation::current_source();
    let path = cache_path(source);
    if due(&read_cache(&path), now())
        && let Ok(executable) = std::env::current_exe()
    {
        let mut command = Command::new(executable);
        command
            .args(["__update-check", source.key()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        // The checker has its own two-second deadline and can finish after the
        // foreground build exits. No download or installation happens here.
        if let Ok(mut child) = command.spawn() {
            // Reap promptly when the parent stays alive for a long build.
            let _ = std::thread::Builder::new()
                .name("cargoc-update-reaper".into())
                .spawn(move || {
                    let _ = child.wait();
                });
        }
    }
    true
}

pub fn show_notice() {
    let source = crate::installation::current_source();
    if let Some(message) = notice(
        &read_cache(&cache_path(source)),
        now(),
        env!("CARGO_PKG_VERSION"),
        source,
    ) {
        eprintln!("{message}");
    }
}

fn fetch_latest(source: InstallSource) -> Result<String> {
    let url = if source == InstallSource::Cargo {
        "https://crates.io/api/v1/crates/cargoc"
    } else {
        "https://api.github.com/repos/bigfish1913/cargoc/releases/latest"
    };
    let response = ureq::AgentBuilder::new()
        .https_only(true)
        .redirects(3)
        .timeout_connect(Duration::from_millis(1000))
        .timeout(Duration::from_millis(1800))
        .build()
        .get(url)
        .set("User-Agent", concat!("cargoc/", env!("CARGO_PKG_VERSION")))
        .set("Accept", "application/json")
        .call()?;
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 1024 * 1024 {
        return fail("Update metadata is too large");
    }
    let value: serde_json::Value = serde_json::from_slice(&bytes)?;
    parse_latest(&value, source)
}
pub fn check_explicit(source: InstallSource) -> Result<()> {
    let latest = fetch_latest(source)?;
    println!(
        "Install source: {}; installed: {}; latest: {latest}",
        source.key(),
        env!("CARGO_PKG_VERSION")
    );
    println!("Update command: {}", source.update_command());
    Ok(())
}

fn parse_latest(value: &serde_json::Value, source: InstallSource) -> Result<String> {
    let candidate = if source == InstallSource::Cargo {
        value
            .get("crate")
            .and_then(|v| v.get("max_stable_version"))
            .and_then(|v| v.as_str())
    } else {
        if value.get("draft").and_then(|v| v.as_bool()) != Some(false)
            || value.get("prerelease").and_then(|v| v.as_bool()) != Some(false)
        {
            return fail("Not a published stable release");
        }
        value
            .get("tag_name")
            .and_then(|v| v.as_str())
            .and_then(|v| v.strip_prefix('v'))
    }
    .ok_or("No stable version in update metadata")?;
    let version = Version::parse(candidate)?;
    if candidate.len() > 128 || !version.pre.is_empty() {
        return fail("Invalid stable release version");
    }
    Ok(candidate.to_owned())
}

fn refresh(path: &Path, time: f64, fetch: impl FnOnce() -> Result<String>) -> Result<()> {
    let _lock = match Lease::acquire(&path.with_extension("lock"), true)? {
        Some(lock) => lock,
        None => return Ok(()),
    };
    let mut cache = read_cache(path);
    if !due(&cache, time) {
        return Ok(());
    }
    cache.attempted_at = time;
    atomic_write(path, &serde_json::to_vec(&cache)?)?;
    if let Ok(latest) = fetch() {
        cache.latest = Some(latest);
        cache.checked_at = time;
        atomic_write(path, &serde_json::to_vec(&cache)?)?;
    }
    Ok(())
}

/// Private helper entry point; failures never propagate to the Cargo invocation.
pub fn run_worker(source: InstallSource) -> Result<()> {
    if disabled() {
        return Ok(());
    }
    let path = cache_path(source);
    // Reject a missing home rather than falling back to writing project files.
    if !crate::config::home().is_absolute() {
        return Ok(());
    }
    refresh(&path, now(), || fetch_latest(source))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }
    #[test]
    fn offline_frozen_machine_and_version_invocations_are_not_eligible() {
        assert!(eligible(&args(&["build"])));
        assert!(eligible(&args(&["+stable", "check"])));
        for values in [
            &["build", "--offline"][..],
            &["test", "--frozen"],
            &["build", "--config", "net.offline=true"],
            &["metadata", "--format-version", "1"],
            &["--version"],
            &["-vV"],
            &["build", "--help"],
        ] {
            assert!(!eligible(&args(values)), "{values:?}");
        }
    }
    #[test]
    fn notices_are_fresh_semver_validated_and_source_aware() {
        let cache = Cache {
            checked_at: 100.0,
            latest: Some("1.2.0".into()),
            ..Cache::default()
        };
        assert!(
            notice(&cache, 101.0, "1.1.0", InstallSource::Homebrew)
                .unwrap()
                .contains("brew upgrade")
        );
        assert!(notice(&cache, 101.0, "1.2.0", InstallSource::Github).is_none());
        assert!(notice(&cache, 101.0, "2.0.0", InstallSource::Github).is_none());
        assert!(notice(&cache, 99.0, "1.0.0", InstallSource::Github).is_none());
        assert!(
            notice(
                &cache,
                100.0 + MAX_AGE + 1.0,
                "1.0.0",
                InstallSource::Github
            )
            .is_none()
        );
    }
    #[test]
    fn failed_checks_preserve_freshness_and_are_rate_limited() {
        let dir = tempfile::tempdir_in(fs::canonicalize(std::env::temp_dir()).unwrap()).unwrap();
        let path = dir.path().join("check.json");
        refresh(&path, 100.0, || Ok("1.2.0".into())).unwrap();
        refresh(&path, 101.0, || panic!("fresh cache must not fetch")).unwrap();
        refresh(&path, 100.0 + INTERVAL + 1.0, || fail("offline")).unwrap();
        let cache = read_cache(&path);
        assert_eq!(cache.checked_at, 100.0);
        assert_eq!(cache.latest.as_deref(), Some("1.2.0"));
        assert!(!due(&cache, 100.0 + INTERVAL + 2.0));
    }
    #[test]
    fn registries_cannot_mix_stable_versions_or_accept_prereleases() {
        use serde_json::json;
        assert_eq!(
            parse_latest(
                &json!({"crate":{"max_stable_version":"1.2.0"}}),
                InstallSource::Cargo
            )
            .unwrap(),
            "1.2.0"
        );
        assert!(
            parse_latest(
                &json!({"tag_name":"v1.2.0", "draft":false,"prerelease":false}),
                InstallSource::Cargo
            )
            .is_err()
        );
        assert!(
            parse_latest(
                &json!({"tag_name":"v1.2.0-rc.1", "draft":false,"prerelease":false}),
                InstallSource::Github
            )
            .is_err()
        );
    }
}
