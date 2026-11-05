use crate::{
    Result, fail,
    util::{absolute, atomic_write, identity_path, same_or_descendant},
};
use serde::{Deserialize, Serialize};
use std::{
    env, fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub cache_dir: PathBuf,
    pub auto_clean: bool,
    pub max_cache_gib: f64,
    pub low_water_ratio: f64,
    pub min_free_gib: f64,
    pub abort_free_gib: f64,
    pub max_age_days: f64,
    pub poll_seconds: f64,
    pub cargo: PathBuf,
}

pub fn home() -> PathBuf {
    env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .map_or_else(|| PathBuf::from("."), PathBuf::from)
}

pub fn default_config_path() -> PathBuf {
    if let Some(path) = env::var_os("CARGOC_CONFIG") {
        return path.into();
    }
    cargo_home().join("cargoc.toml")
}

pub fn cargo_home() -> PathBuf {
    env::var_os("CARGO_HOME")
        .filter(|path| !path.is_empty())
        .map_or_else(|| home().join(".cargo"), PathBuf::from)
}

/// Cargo's user-level configuration file, preferring the modern name.
pub fn cargo_config_path() -> PathBuf {
    let home = cargo_home();
    let toml = home.join("config.toml");
    if toml.is_file() {
        toml
    } else {
        home.join("config")
    }
}

fn legacy_build_dir_value(value: &str, cache_dir: &Path) -> Result<bool> {
    // Only offer removal for absolute workspace-hash build directories. A
    // user's unrelated static build-dir is not silently classified as legacy.
    if !Path::new(value).is_absolute() || !value.contains("{workspace-path-hash}") {
        return Ok(false);
    }
    let Some(base) = value
        .strip_suffix("/{workspace-path-hash}")
        .or_else(|| value.strip_suffix("\\{workspace-path-hash}"))
    else {
        return Ok(false);
    };
    let overlaps = same_or_descendant(Path::new(base), cache_dir)?
        || same_or_descendant(cache_dir, Path::new(base))?;
    Ok(!overlaps)
}

/// Return the external dynamic build-dir that setup can offer to remove.
pub fn legacy_build_dir(path: &Path, cache_dir: &Path) -> Result<Option<String>> {
    if !path.is_file() {
        return Ok(None);
    }
    let data: toml::Value = toml::from_str(&fs::read_to_string(path)?)?;
    let Some(value) = data
        .get("build")
        .and_then(|section| section.get("build-dir"))
        .and_then(toml::Value::as_str)
    else {
        return Ok(None);
    };
    if legacy_build_dir_value(value, cache_dir)? {
        Ok(Some(value.to_owned()))
    } else {
        Ok(None)
    }
}

/// Remove only a detected legacy dynamic build-dir after creating a backup.
/// Other Cargo settings, comments and line endings are preserved.
pub fn remove_legacy_build_dir(path: &Path, cache_dir: &Path) -> Result<Option<PathBuf>> {
    let Some(expected) = legacy_build_dir(path, cache_dir)? else {
        return Ok(None);
    };
    let source = fs::read_to_string(path)?;
    let mut output = String::with_capacity(source.len());
    let mut in_build = false;
    let mut removed = false;
    for segment in source.split_inclusive('\n') {
        let line = segment.trim_end_matches(['\n', '\r']);
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            in_build = trimmed == "[build]";
        }
        let is_build_dir = in_build
            && trimmed
                .split_once('=')
                .is_some_and(|(key, _)| key.trim() == "build-dir");
        if is_build_dir {
            let value = trimmed.split_once('=').map(|(_, value)| value.trim());
            if value.is_some_and(|value| {
                toml::from_str::<toml::Value>(&format!("build-dir = {value}"))
                    .ok()
                    .and_then(|data| {
                        data.get("build-dir")
                            .and_then(toml::Value::as_str)
                            .map(str::to_owned)
                    })
                    .is_some_and(|value| value == expected)
            }) {
                removed = true;
                continue;
            }
        }
        output.push_str(segment);
    }
    if !removed {
        return fail("Legacy Cargo build-dir changed while setup was running");
    }
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    let backup = path.with_file_name(format!(
        "{}.cargoc-backup-{nonce}",
        path.file_name()
            .ok_or("Cargo config has no filename")?
            .to_string_lossy()
    ));
    fs::copy(path, &backup)?;
    atomic_write(path, output.as_bytes())?;
    Ok(Some(backup))
}

/// Location used by cargoc 0.1.0; retained for one-time migration.
pub fn legacy_config_path() -> PathBuf {
    let base = env::var_os(if cfg!(windows) {
        "APPDATA"
    } else {
        "XDG_CONFIG_HOME"
    })
    .map_or_else(|| home().join(".config"), PathBuf::from);
    base.join("cargoc").join("config.toml")
}

/// Copy validated legacy settings, retaining the original as a recovery copy.
/// Existing/new explicit configurations are never overwritten or merged.
pub fn migrate_legacy_config(destination: &Path, legacy: &Path) -> Result<bool> {
    if destination.exists() || !legacy.is_file() {
        return Ok(false);
    }
    let config = Config::load(legacy)?;
    if crate::util::same_or_descendant(destination, &config.cache_dir)? {
        return fail("Config must be outside the disposable cache root");
    }
    crate::util::check_path(destination)?;
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    let stage = destination.with_file_name(format!(
        ".cargoc-migrate-{}-{nonce}.toml",
        std::process::id()
    ));
    config.save(&stage)?;
    // Creating a hard link publishes the complete file without replacing a
    // config concurrently created by another invocation. Both paths share a disk.
    let result = match fs::hard_link(&stage, destination) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        Err(error) => Err(error.into()),
    };
    let _ = fs::remove_file(&stage);
    result
}

impl Default for Config {
    fn default() -> Self {
        let base = env::var_os(if cfg!(windows) {
            "LOCALAPPDATA"
        } else {
            "XDG_CACHE_HOME"
        })
        .map_or_else(|| home().join(".cache"), PathBuf::from);
        Self {
            cache_dir: base.join("cargoc/cache"),
            auto_clean: true,
            max_cache_gib: 80.0,
            low_water_ratio: 0.8,
            min_free_gib: 20.0,
            abort_free_gib: 1.0,
            max_age_days: 7.0,
            poll_seconds: 5.0,
            cargo: "cargo".into(),
        }
    }
}

impl Config {
    pub fn validate(&self) -> Result<()> {
        for (key, value) in [
            ("max_cache_gib", self.max_cache_gib),
            ("low_water_ratio", self.low_water_ratio),
            ("min_free_gib", self.min_free_gib),
            ("abort_free_gib", self.abort_free_gib),
            ("max_age_days", self.max_age_days),
            ("poll_seconds", self.poll_seconds),
        ] {
            if !value.is_finite() || value < 0.0 {
                return fail(format!("{key} must be finite and nonnegative"));
            }
        }
        if self.max_cache_gib <= 0.0
            || !(0.0..1.0).contains(&self.low_water_ratio)
            || self.low_water_ratio == 0.0
        {
            return fail("max_cache_gib must be positive; low_water_ratio must be between 0 and 1");
        }
        if !(0.1..=86400.0).contains(&self.poll_seconds) {
            return fail("poll_seconds must be between 0.1 and 86400");
        }
        for capacity in [self.max_cache_gib, self.min_free_gib, self.abort_free_gib] {
            if capacity > u64::MAX as f64 / crate::util::GIB {
                return fail("Space threshold is too large to represent in bytes");
            }
        }
        if self.min_free_gib > 0.0 && self.abort_free_gib > self.min_free_gib {
            return fail("abort_free_gib cannot exceed min_free_gib");
        }
        if self.cache_dir.as_os_str().is_empty() || self.cargo.as_os_str().is_empty() {
            return fail("cache_dir and cargo cannot be empty");
        }
        let root = absolute(&self.cache_dir)?;
        if root.parent().is_none() || identity_path(&root)? == identity_path(&home())? {
            return fail("Use a dedicated cache directory, not a drive root/home");
        }
        if !self.cache_dir.is_absolute() {
            return fail("cache_dir must be an absolute path");
        }
        Ok(())
    }

    pub fn load(path: &Path) -> Result<Self> {
        let text = fs::read_to_string(path)
            .map_err(|e| format!("Cannot read {}: {e}; run cargoc init", path.display()))?;
        let config: Self = toml::from_str(&text)?;
        config.validate()?;
        Ok(config)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        self.validate()?;
        atomic_write(path, toml::to_string_pretty(self)?.as_bytes())
    }
}
