//! Install-channel discovery is advisory; it never invokes a package manager.
use std::{fs, path::Path};

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum InstallSource {
    Github,
    Cargo,
    Homebrew,
    Scoop,
}

impl InstallSource {
    pub fn key(self) -> &'static str {
        match self {
            Self::Github => "github",
            Self::Cargo => "cargo",
            Self::Homebrew => "homebrew",
            Self::Scoop => "scoop",
        }
    }
    pub fn update_command(self) -> &'static str {
        match self {
            Self::Github => "cargoc self-update",
            Self::Cargo => "cargo install cargoc --locked",
            Self::Homebrew => "brew update && brew upgrade bigfish1913/tap/cargoc",
            Self::Scoop => "scoop update && scoop update cargoc",
        }
    }
    pub fn package_managed(self) -> bool {
        self != Self::Github
    }
}

pub fn current_source() -> InstallSource {
    std::env::current_exe()
        .map(|path| detect(&path))
        .unwrap_or(InstallSource::Github)
}

fn path_source(path: &Path) -> Option<InstallSource> {
    let text = path.to_string_lossy().replace('\\', "/").to_lowercase();
    if text.contains("/cellar/cargoc/") {
        Some(InstallSource::Homebrew)
    } else if text.contains("/apps/cargoc/") {
        Some(InstallSource::Scoop)
    } else {
        None
    }
}

fn detect(path: &Path) -> InstallSource {
    let resolved = fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    if let Some(source) = path_source(&resolved).or_else(|| path_source(path)) {
        return source;
    }
    // Cargo records registry installs separately from local path/git builds.
    // Read only the install root belonging to this executable, not an arbitrary
    // project file or a different CARGO_HOME selected by the current shell.
    if let Some(root) = resolved
        .parent()
        .filter(|p| p.file_name().is_some_and(|n| n == "bin"))
        .and_then(Path::parent)
    {
        let file = root.join(".crates2.json");
        if let Ok(bytes) = crate::util::read_bounded(&file, 1024 * 1024)
            && let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes)
            && value
                .get("installs")
                .and_then(|v| v.as_object())
                .is_some_and(|installs| {
                    installs
                        .keys()
                        .any(|key| key.starts_with("cargoc ") && key.contains("(registry+"))
                })
        {
            return InstallSource::Cargo;
        }
    }
    InstallSource::Github
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn package_paths_produce_channel_specific_commands() {
        assert_eq!(
            path_source(Path::new("/opt/homebrew/Cellar/cargoc/0.1.1/bin/cargoc")),
            Some(InstallSource::Homebrew)
        );
        assert_eq!(
            path_source(Path::new("C:\\Scoop\\apps\\cargoc\\current\\cargoc.exe")),
            Some(InstallSource::Scoop)
        );
        assert_eq!(path_source(Path::new("/home/me/.local/bin/cargoc")), None);
        assert!(
            InstallSource::Scoop
                .update_command()
                .contains("scoop update cargoc")
        );
    }
    #[test]
    fn registry_installs_are_distinct_from_path_builds() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        fs::create_dir(&bin).unwrap();
        let exe = bin.join("cargoc");
        fs::write(&exe, b"fake").unwrap();
        let record = dir.path().join(".crates2.json");
        fs::write(&record, br#"{"installs":{"cargoc 0.1.1 (registry+https://github.com/rust-lang/crates.io-index)":{}}}"#).unwrap();
        assert_eq!(detect(&exe), InstallSource::Cargo);
        fs::write(
            &record,
            br#"{"installs":{"cargoc 0.1.1 (path+file:///source)":{}}}"#,
        )
        .unwrap();
        assert_eq!(detect(&exe), InstallSource::Github);
    }
}
