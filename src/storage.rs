//! Opt-in fail-closed storage binding. Device numbers detect mount boundaries,
//! not permanent volume identity; the bound ownership marker identifies the cache.
use crate::{
    Result,
    config::Config,
    fail,
    util::{absolute, check_path, read_bounded},
};
use std::path::Path;

pub fn root_id(root: &Path) -> Result<String> {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Marker {
        version: u32,
        root_id: String,
    }
    let marker: Marker =
        serde_json::from_slice(&read_bounded(&root.join(".cargoc-root.json"), 4096)?)?;
    if marker.version != 1 || !valid_id(&marker.root_id) {
        return fail("Invalid cache ownership marker for storage binding");
    }
    Ok(marker.root_id)
}

fn valid_id(id: &str) -> bool {
    id.len() == 64
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Structural validation only: loading configuration must work when a disk is absent.
pub fn validate_settings(config: &Config) -> Result<()> {
    match (&config.require_mount, &config.required_root_id) {
        (None, None) => return Ok(()),
        (Some(mount), Some(id)) => {
            if !cfg!(unix) {
                return fail("require_mount is supported only on Unix");
            }
            if !mount.is_absolute() || absolute(mount)?.parent().is_none() || !valid_id(id) {
                return fail(
                    "require_mount needs a non-root absolute mount path and a valid required_root_id",
                );
            }
            let root = absolute(&config.cache_dir)?;
            let mount = absolute(mount)?;
            if root == mount || !root.starts_with(&mount) {
                return fail("cache_dir must be strictly inside require_mount");
            }
        }
        _ => {
            return fail(
                "require_mount and required_root_id must be configured together; use config --require-mount",
            );
        }
    }
    Ok(())
}

#[cfg(unix)]
fn device(path: &Path) -> Result<u64> {
    use std::os::unix::fs::MetadataExt;
    Ok(std::fs::metadata(path)?.dev())
}

#[cfg(not(unix))]
fn device(_path: &Path) -> Result<u64> {
    fail("require_mount is supported only on Unix")
}

pub fn check(config: &Config) -> Result<()> {
    check_with_device(config, device)
        .map_err(|e| format!("Cache storage unavailable (no fallback): {e}").into())
}

fn check_with_device(config: &Config, device: impl Fn(&Path) -> Result<u64>) -> Result<()> {
    validate_settings(config)?;
    let Some(mount) = &config.require_mount else {
        return Ok(());
    };
    let mount = absolute(mount)?;
    let root = absolute(&config.cache_dir)?;
    check_path(&mount)?;
    check_path(&root)?;
    if !mount.is_dir() || !root.is_dir() {
        return fail("Required mount/cache directory is missing; reconnect the bound volume");
    }
    let dev = device(&mount)?;
    if dev == device(mount.parent().ok_or("Mount has no parent")?)? {
        return fail("require_mount is an ordinary directory, not a separate mounted filesystem");
    }
    if dev != device(&root)? {
        return fail("Cache directory is not on the required mounted filesystem");
    }
    if Some(root_id(&root)?) != config.required_root_id {
        return fail(
            "Cache ownership differs from the bound root; refusing to initialize or adopt it",
        );
    }
    Ok(())
}

/// Bind only an already initialized cache on a currently mounted volume.
pub fn bind(config: &mut Config, mount: &Path) -> Result<()> {
    let mut next = config.clone();
    next.require_mount = Some(absolute(mount)?);
    next.required_root_id = Some(root_id(&next.cache_dir)?);
    check(&next)?;
    *config = next;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;
    use std::fs;

    #[test]
    fn injected_mount_identity_and_device_changes_fail_closed() {
        let temp = tempfile::tempdir().unwrap();
        let mount = fs::canonicalize(temp.path()).unwrap().join("volume");
        let mut cfg = Config {
            cache_dir: mount.join("cache"),
            ..Config::default()
        };
        Store::open(cfg.clone(), true).unwrap();
        cfg.require_mount = Some(mount.clone());
        cfg.required_root_id = Some(root_id(&cfg.cache_dir).unwrap());
        let mounted = |p: &Path| Ok(if p.starts_with(&mount) { 2 } else { 1 });
        if cfg!(unix) {
            check_with_device(&cfg, mounted).unwrap();
        }
        assert!(check_with_device(&cfg, |_| Ok(1)).is_err());
        assert!(check_with_device(&cfg, |p| Ok(if p == mount { 2 } else { 1 })).is_err());
        cfg.required_root_id = Some("a".repeat(64));
        assert!(check_with_device(&cfg, mounted).is_err());
        fs::remove_dir_all(&cfg.cache_dir).unwrap();
        assert!(check_with_device(&cfg, mounted).is_err());
        assert!(!cfg.cache_dir.exists());
    }
}
