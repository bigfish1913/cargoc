//! Optional user-level Cargo integration. The real rustup Cargo binary is never replaced.
use crate::{
    config::{Config, home},
    fail,
    util::{absolute, atomic_write, check_path},
};
use serde::{Deserialize, Serialize};
#[cfg(windows)]
use std::process::Command;
use std::{
    env, fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IntegrationState {
    pub shim_dir: PathBuf,
    pub real_cargo: PathBuf,
    pub enabled: bool,
}

pub fn state_path() -> PathBuf {
    home().join(".cargoc/integration.toml")
}
pub fn shim_dir() -> PathBuf {
    home().join(".cargoc/bin")
}

fn path_candidates(program: &Path) -> Vec<PathBuf> {
    let mut result = Vec::new();
    if program.is_absolute() || program.components().count() > 1 {
        result.push(program.to_path_buf());
    } else if let Some(path) = env::var_os("PATH") {
        for dir in env::split_paths(&path) {
            result.push(dir.join(program));
            #[cfg(windows)]
            if program.extension().is_none() {
                result.push(dir.join("cargo.exe"));
            }
        }
    }
    result
}

pub fn find_real_cargo(exclude: Option<&Path>) -> crate::Result<PathBuf> {
    let mut candidates = Vec::new();
    #[cfg(windows)]
    if let Some(user) = env::var_os("USERPROFILE") {
        candidates.push(PathBuf::from(user).join(".cargo/bin/cargo.exe"));
    }
    candidates.extend(path_candidates(Path::new("cargo")));
    let self_exe = fs::canonicalize(std::env::current_exe()?).ok();
    for candidate in candidates {
        if let Some(exclude) = exclude
            && crate::util::same_or_descendant(&candidate, exclude)?
        {
            continue;
        }
        let Ok(path) = fs::canonicalize(&candidate) else {
            continue;
        };
        if self_exe
            .as_ref()
            .is_some_and(|self_path| *self_path == path)
        {
            continue;
        }
        if path
            .file_stem()
            .is_some_and(|name| name.eq_ignore_ascii_case("cargo"))
        {
            return Ok(path);
        }
    }
    fail("Cannot find the real Cargo executable. Install Rust through rustup, or pass --cargo PATH")
}

fn load_state() -> crate::Result<Option<IntegrationState>> {
    let path = state_path();
    if !path.exists() {
        return Ok(None);
    }
    Ok(Some(toml::from_str(&fs::read_to_string(path)?)?))
}

fn save_state(state: &IntegrationState) -> crate::Result<()> {
    atomic_write(&state_path(), toml::to_string_pretty(state)?.as_bytes())
}

#[cfg(windows)]
fn shim_path() -> PathBuf {
    shim_dir().join("cargo.cmd")
}
#[cfg(not(windows))]
fn shim_path() -> PathBuf {
    shim_dir().join("cargo")
}

fn write_shim(executable: &Path, config_path: &Path) -> crate::Result<()> {
    let executable = executable
        .to_str()
        .ok_or("cargoc path is not valid UTF-8")?;
    check_path(&shim_dir())?;
    fs::create_dir_all(shim_dir())?;
    let config_path = absolute(config_path)?;
    let config = config_path
        .to_str()
        .ok_or("Config path is not valid UTF-8")?;
    #[cfg(windows)]
    let content = format!(
        "@echo off\r\n\"{}\" --cargoc-config \"{}\" exec %*\r\n",
        executable, config
    );
    #[cfg(not(windows))]
    let content = format!(
        "#!/bin/sh\nexec \"{}\" --cargoc-config \"{}\" exec \"$@\"\n",
        executable, config
    );
    atomic_write(&shim_path(), content.as_bytes())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(shim_path())?.permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(shim_path(), permissions)?;
    }
    Ok(())
}

#[cfg(windows)]
fn user_path() -> crate::Result<Option<(String, String)>> {
    let output = Command::new("reg")
        .args(["query", "HKCU\\Environment", "/v", "Path"])
        .output()?;
    if !output.status.success() {
        return Ok(None);
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let line = text
        .lines()
        .find(|line| line.contains(" Path "))
        .ok_or("User PATH registry value is malformed")?;
    let mut parts = line
        .splitn(3, char::is_whitespace)
        .filter(|part| !part.is_empty());
    let _ = parts.next();
    let kind = parts.next().ok_or("User PATH type missing")?;
    let value = parts.next().unwrap_or("").to_string();
    Ok(Some((kind.to_string(), value)))
}

#[cfg(windows)]
fn set_user_path(value: &str, kind: &str) -> crate::Result<()> {
    let status = Command::new("reg")
        .args([
            "add",
            "HKCU\\Environment",
            "/v",
            "Path",
            "/t",
            kind,
            "/d",
            value,
            "/f",
        ])
        .status()?;
    if !status.success() {
        return fail("Unable to update the user PATH registry value");
    }
    Ok(())
}

#[cfg(windows)]
fn enable_path() -> crate::Result<()> {
    let (kind, value) = user_path()?.unwrap_or(("REG_EXPAND_SZ".into(), String::new()));
    let shim = shim_dir().to_string_lossy().to_string();
    let pieces: Vec<_> = value
        .split(';')
        .filter(|part| !part.trim().is_empty())
        .collect();
    if pieces.iter().any(|part| part.eq_ignore_ascii_case(&shim)) {
        return Ok(());
    }
    let new_value = if value.trim().is_empty() {
        shim
    } else {
        format!("{shim};{value}")
    };
    set_user_path(&new_value, &kind)
}

#[cfg(windows)]
fn disable_path() -> crate::Result<()> {
    let Some((kind, value)) = user_path()? else {
        return Ok(());
    };
    let shim = shim_dir().to_string_lossy().to_string();
    let new_value = value
        .split(';')
        .filter(|part| !part.eq_ignore_ascii_case(&shim))
        .collect::<Vec<_>>()
        .join(";");
    set_user_path(&new_value, &kind)
}

#[cfg(not(windows))]
fn enable_path() -> crate::Result<()> {
    // Do not silently edit shell startup files. The printed command is safe to
    // inspect and works for the current shell without guessing the user's shell.
    Ok(())
}
#[cfg(not(windows))]
fn disable_path() -> crate::Result<()> {
    Ok(())
}

pub fn enable(
    mut config: Config,
    config_path: &Path,
    explicit_cargo: Option<PathBuf>,
) -> crate::Result<()> {
    let real = if let Some(path) = explicit_cargo {
        absolute(&path)?
    } else {
        find_real_cargo(Some(&shim_dir()))?
    };
    if !real.is_file() {
        return fail(format!(
            "Cargo executable does not exist: {}",
            real.display()
        ));
    }
    config.cargo = real.clone();
    config.validate()?;
    if crate::util::same_or_descendant(config_path, &config.cache_dir)? {
        return fail("Config must be outside cache_dir");
    }
    crate::store::Store::open(config.clone(), true)?;
    config.save(config_path)?;
    write_shim(&absolute(&std::env::current_exe()?)?, config_path)?;
    enable_path()?;
    save_state(&IntegrationState {
        shim_dir: shim_dir(),
        real_cargo: real,
        enabled: true,
    })?;
    println!("cargoc default Cargo integration enabled.");
    println!("Shim: {}", shim_path().display());
    #[cfg(windows)]
    println!("Reopen your terminal for the user PATH change to take effect.");
    #[cfg(not(windows))]
    println!("Add this directory before the Rustup bin directory in your shell PATH:");
    #[cfg(not(windows))]
    println!("  export PATH=\"{}:$PATH\"", shim_dir().display());
    Ok(())
}

pub fn disable() -> crate::Result<()> {
    if let Some(state) = load_state()? {
        disable_path()?;
        if shim_path().exists() {
            check_path(&shim_path())?;
            fs::remove_file(shim_path())?;
        }
        save_state(&IntegrationState {
            enabled: false,
            ..state
        })?;
        println!(
            "cargoc default Cargo integration disabled; the original Cargo path is unchanged."
        );
        #[cfg(windows)]
        println!("Reopen your terminal for the user PATH restoration to take effect.");
    } else {
        println!("No cargoc default Cargo integration is registered.");
    }
    Ok(())
}

pub fn status() -> crate::Result<()> {
    match load_state()? {
        Some(state) if state.enabled => println!(
            "enabled: shim={} real_cargo={}",
            shim_path().display(),
            state.real_cargo.display()
        ),
        _ => println!("disabled"),
    }
    Ok(())
}
