//! Safe, opt-in updates from the project's GitHub Releases.
//!
//! The updater downloads a platform-specific archive, verifies the release
//! checksum and embedded binary version, then asks `self-replace` to atomically
//! replace this executable. Cargo, its configuration, and the managed cache are
//! never modified.
use crate::{Result, fail};
use flate2::read::GzDecoder;
use semver::Version;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    env,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    process::Command,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tar::Archive;
use zip::ZipArchive;

const API_URL: &str = "https://api.github.com/repos/bigfish1913/cargoc/releases/latest";
const MAX_DOWNLOAD: u64 = 128 * 1024 * 1024;
const MAX_BINARY: u64 = 64 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Release {
    tag: String,
    version: String,
    archive: String,
    archive_url: String,
    checksum_url: String,
}

fn target() -> Result<&'static str> {
    if cfg!(target_env = "musl") || cfg!(all(windows, not(target_env = "msvc"))) {
        return fail(
            "No compatible prebuilt cargoc release for this target ABI; use cargo install",
        );
    }
    match (env::consts::OS, env::consts::ARCH) {
        ("windows", "x86_64") => Ok("x86_64-pc-windows-msvc"),
        ("windows", "aarch64") => Ok("aarch64-pc-windows-msvc"),
        ("linux", "x86_64") => Ok("x86_64-unknown-linux-gnu"),
        ("linux", "aarch64") => Ok("aarch64-unknown-linux-gnu"),
        ("macos", "x86_64") => Ok("x86_64-apple-darwin"),
        ("macos", "aarch64") => Ok("aarch64-apple-darwin"),
        (os, arch) => fail(format!("No cargoc release is published for {os}/{arch}")),
    }
}

fn response(url: &str, accept: &str) -> Result<ureq::Response> {
    let agent = ureq::AgentBuilder::new()
        .https_only(true)
        .redirects(5)
        .timeout_connect(Duration::from_secs(10))
        .timeout(Duration::from_secs(60))
        .build();
    match agent
        .get(url)
        .set("Accept", accept)
        .set("User-Agent", concat!("cargoc/", env!("CARGO_PKG_VERSION")))
        .call()
    {
        Ok(response) => Ok(response),
        Err(ureq::Error::Status(404, _)) if url == API_URL => {
            fail("No published cargoc GitHub Release is available yet")
        }
        Err(error) => Err(error.into()),
    }
}

fn request(url: &str) -> Result<Vec<u8>> {
    let response = response(url, "application/vnd.github+json")?;
    let mut reader = response.into_reader().take(1024 * 1024 + 1);
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes)?;
    if bytes.len() > 1024 * 1024 {
        return fail("GitHub response exceeds the safe download limit");
    }
    Ok(bytes)
}

fn valid_version(value: &str) -> bool {
    value.len() <= 128 && Version::parse(value).is_ok()
}

fn latest_release() -> Result<Release> {
    parse_release(&serde_json::from_slice(&request(API_URL)?)?)
}

fn parse_release(data: &Value) -> Result<Release> {
    if data.get("draft").and_then(Value::as_bool) != Some(false)
        || data.get("prerelease").and_then(Value::as_bool) != Some(false)
    {
        return fail("GitHub did not return a published stable release");
    }
    let tag = data
        .get("tag_name")
        .and_then(Value::as_str)
        .ok_or("GitHub release has no tag")?;
    let version = tag
        .strip_prefix('v')
        .ok_or("GitHub release tag is not a v-prefixed version")?;
    if !valid_version(version) {
        return fail("GitHub release has an unsafe or invalid version tag");
    }
    let target = target()?;
    let archive = format!(
        "cargoc-v{version}-{target}.{}",
        if cfg!(windows) { "zip" } else { "tar.gz" }
    );
    let assets = data
        .get("assets")
        .and_then(Value::as_array)
        .ok_or("GitHub release has no assets")?;
    let asset_url = |name: &str| {
        let expected =
            format!("https://github.com/bigfish1913/cargoc/releases/download/{tag}/{name}");
        let matching: Vec<_> = assets
            .iter()
            .filter(|asset| asset.get("name").and_then(Value::as_str) == Some(name))
            .collect();
        (matching.len() == 1
            && matching[0]
                .get("browser_download_url")
                .and_then(Value::as_str)
                == Some(expected.as_str()))
        .then_some(expected)
    };
    Ok(Release {
        tag: tag.to_owned(),
        version: version.to_owned(),
        archive: archive.clone(),
        archive_url: asset_url(&archive).ok_or(format!("Release is missing {archive}"))?,
        checksum_url: asset_url(&format!("{archive}.sha256"))
            .ok_or(format!("Release is missing {archive}.sha256"))?,
    })
}

fn newer(candidate: &str, current: &str) -> Result<bool> {
    Ok(Version::parse(candidate)?
        .cmp_precedence(&Version::parse(current)?)
        .is_gt())
}

fn checksum(text: &[u8], archive: &str) -> Result<[u8; 32]> {
    if text.len() > 4096 {
        return fail("Checksum file is too large");
    }
    let line = std::str::from_utf8(text)?;
    if line.split_whitespace().count() != 2 {
        return fail("Invalid checksum file");
    }
    let digest = line
        .split_whitespace()
        .next()
        .ok_or("Empty checksum file")?;
    let listed_name = line
        .split_whitespace()
        .nth(1)
        .unwrap_or("")
        .trim_start_matches('*');
    if listed_name != archive
        || digest.len() != 64
        || !digest.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return fail("Release checksum does not identify the expected archive");
    }
    let mut result = [0_u8; 32];
    for index in 0..32 {
        result[index] = u8::from_str_radix(&digest[index * 2..index * 2 + 2], 16)?;
    }
    Ok(result)
}

fn download(path: &Path, url: &str) -> Result<()> {
    let response = response(url, "application/octet-stream")?;
    let mut reader = response.into_reader().take(MAX_DOWNLOAD + 1);
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    let mut bytes = [0_u8; 64 * 1024];
    let mut total = 0_u64;
    loop {
        let count = reader.read(&mut bytes)?;
        if count == 0 {
            break;
        }
        total += count as u64;
        if total > MAX_DOWNLOAD {
            return fail("Release archive exceeds the safe download limit");
        }
        file.write_all(&bytes[..count])?;
    }
    file.sync_all()?;
    Ok(())
}

fn binary_name() -> &'static str {
    if cfg!(windows) {
        "cargoc.exe"
    } else {
        "cargoc"
    }
}

fn safe_member(path: &Path, expected_root: &str) -> bool {
    let mut components = path.components();
    matches!(components.next(), Some(Component::Normal(root)) if root == expected_root)
        && matches!(components.next(), Some(Component::Normal(name)) if name == binary_name())
        && components.next().is_none()
}

fn extract_binary(archive: &Path, release: &Release, destination: &Path) -> Result<()> {
    let root = format!("cargoc-v{}-{}", release.version, target()?);
    let mut binary = None;
    if cfg!(windows) {
        let file = File::open(archive)?;
        let mut zip = ZipArchive::new(file)?;
        for index in 0..zip.len() {
            let mut entry = zip.by_index(index)?;
            if safe_member(Path::new(entry.name()), &root) {
                if binary.is_some() {
                    return fail("Release archive contains duplicate cargoc binaries");
                }
                if entry.is_dir()
                    || entry
                        .unix_mode()
                        .is_some_and(|mode| mode & 0o170000 != 0 && mode & 0o170000 != 0o100000)
                {
                    return fail("Release binary entry is not a regular file");
                }
                let mut bytes = Vec::new();
                entry
                    .by_ref()
                    .take(MAX_BINARY + 1)
                    .read_to_end(&mut bytes)?;
                if bytes.len() as u64 > MAX_BINARY {
                    return fail("Release binary is too large");
                }
                binary = Some(bytes);
            }
        }
    } else {
        let file = File::open(archive)?;
        let decoder = GzDecoder::new(file).take(MAX_DOWNLOAD + 1);
        let mut tar = Archive::new(decoder);
        for entry in tar.entries()? {
            let entry = entry?;
            let member = entry.path()?.into_owned();
            if safe_member(&member, &root) {
                if binary.is_some() || !entry.header().entry_type().is_file() {
                    return fail("Invalid release binary entry");
                }
                let mut bytes = Vec::new();
                entry.take(MAX_BINARY + 1).read_to_end(&mut bytes)?;
                if bytes.len() as u64 > MAX_BINARY {
                    return fail("Release binary is too large");
                }
                binary = Some(bytes);
            }
        }
    }
    let bytes = binary.ok_or("Release archive does not contain the expected cargoc binary")?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = file.metadata()?.permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(destination, permissions)?;
    }
    Ok(())
}

fn validate_binary(path: &Path, expected: Option<&str>) -> Result<()> {
    let output = Command::new(path).arg("--version").output()?;
    let actual = String::from_utf8_lossy(&output.stdout);
    let valid = actual
        .trim()
        .strip_prefix("cargoc ")
        .is_some_and(|version| expected.is_none_or(|expected| version == expected));
    if !output.status.success() || !valid {
        return fail(format!(
            "Downloaded binary version mismatch: {}",
            actual.trim()
        ));
    }
    Ok(())
}

fn rollback_dir(executable: &Path) -> Result<PathBuf> {
    let identity = crate::util::identity_path(executable)?;
    let key = format!("{:x}", Sha256::digest(identity.as_bytes()));
    Ok(crate::config::home()
        .join(".cargoc")
        .join("rollback")
        .join(key))
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Backup {
    file: String,
    sha256: String,
    version: String,
}

fn backup_path(directory: &Path, backup: &Backup) -> Result<PathBuf> {
    if !backup.file.starts_with("cargoc-")
        || backup.file.contains(['/', '\\', ':'])
        || !valid_version(&backup.version)
    {
        return fail("Invalid rollback metadata");
    }
    let path = directory.join(&backup.file);
    crate::util::check_path(&path)?;
    if !path.is_file() || format!("{:x}", Sha256::digest(fs::read(&path)?)) != backup.sha256 {
        return fail("Rollback backup SHA-256 verification failed");
    }
    Ok(path)
}

fn nonce() -> Result<String> {
    Ok(format!(
        "{}-{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
    ))
}

fn replace(executable: &Path, replacement: &Path, label: &str) -> Result<()> {
    crate::util::check_path(executable)?;
    let backup_dir = rollback_dir(executable)?;
    crate::util::check_path(&backup_dir)?;
    fs::create_dir_all(&backup_dir)?;
    let file = format!("cargoc-{}{}", nonce()?, env::consts::EXE_SUFFIX);
    let backup = backup_dir.join(&file);
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&backup)?;
    std::io::copy(&mut File::open(executable)?, &mut output)?;
    output.sync_all()?;
    drop(output);
    fs::set_permissions(&backup, fs::metadata(executable)?.permissions())?;
    let metadata = Backup {
        file,
        sha256: format!("{:x}", Sha256::digest(fs::read(&backup)?)),
        version: env!("CARGO_PKG_VERSION").to_owned(),
    };
    // Persist recovery information before attempting a platform-specific replacement.
    crate::util::atomic_write(
        &backup_dir.join("latest.json"),
        &serde_json::to_vec(&metadata)?,
    )?;
    if let Err(error) = self_replace::self_replace(replacement) {
        // Windows can move the running executable away before a subsequent copy fails.
        // Keep the durable backup even when replacement fails, and restore a missing path.
        if !executable.exists() {
            fs::copy(&backup, executable).map_err(|restore| {
                format!(
                    "Replacement failed: {error}; restoration failed: {restore}; recover from {}",
                    backup.display()
                )
            })?;
        }
        return Err(error.into());
    }
    println!(
        "Updated cargoc to {label}; rollback backup: {}",
        backup.display()
    );
    Ok(())
}

pub fn run(check_only: bool, rollback: bool, force: bool) -> Result<()> {
    if rollback && (check_only || force) {
        return fail("--rollback conflicts with --check/--force");
    }
    let source = crate::installation::current_source();
    if source.package_managed() {
        if check_only {
            return crate::updates::check_explicit(source);
        }
        return fail(format!(
            "This installation is managed by {}; use: {}",
            source.key(),
            source.update_command()
        ));
    }
    let executable = env::current_exe()?;
    let directory = rollback_dir(&executable)?;
    // Read-only checks need neither an installation lock nor a writable home.
    let _lock = if check_only {
        None
    } else {
        Some(
            crate::store::Lease::acquire(&directory.join("update.lock"), true)?
                .ok_or("Another cargoc update is running for this installation")?,
        )
    };
    if rollback {
        crate::util::check_path(&directory.join("latest.json"))?;
        let backup: Backup = serde_json::from_slice(&fs::read(directory.join("latest.json"))?)?;
        let path = backup_path(&directory, &backup)?;
        validate_binary(&path, Some(&backup.version))?;
        replace(&executable, &path, &backup.version)?;
        return Ok(());
    }
    let release = latest_release()?;
    let current = env!("CARGO_PKG_VERSION");
    if !force && !newer(&release.version, current)? {
        println!(
            "cargoc is already up to date ({current}); latest release is {}",
            release.tag
        );
        return Ok(());
    }
    println!("Latest release: {}; installed: {}", release.tag, current);
    if check_only {
        return Ok(());
    }
    let temporary = tempfile::Builder::new()
        .prefix("cargoc-update-")
        .tempdir()?;
    let temporary = temporary.path();
    let archive = temporary.join(&release.archive);
    let checksum_file = temporary.join("checksum.sha256");
    download(&checksum_file, &release.checksum_url)?;
    let expected = checksum(&fs::read(&checksum_file)?, &release.archive)?;
    download(&archive, &release.archive_url)?;
    let actual = Sha256::digest(fs::read(&archive)?);
    if actual.as_slice() != expected {
        return fail("Release archive SHA-256 verification failed");
    }
    let replacement = temporary.join(binary_name());
    extract_binary(&archive, &release, &replacement)?;
    validate_binary(&replacement, Some(&release.version))?;
    replace(&executable, &replacement, &release.version)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release_data() -> Value {
        let archive = format!(
            "cargoc-v1.2.3-{}.{}",
            target().unwrap(),
            if cfg!(windows) { "zip" } else { "tar.gz" }
        );
        serde_json::json!({"tag_name": "v1.2.3", "draft": false, "prerelease": false,
        "assets": [
            {"name": archive, "browser_download_url": format!("https://github.com/bigfish1913/cargoc/releases/download/v1.2.3/{archive}")},
            {"name": format!("{archive}.sha256"), "browser_download_url": format!("https://github.com/bigfish1913/cargoc/releases/download/v1.2.3/{archive}.sha256")}
        ]})
    }

    #[test]
    fn version_precedence_never_downgrades_without_force() {
        assert!(newer("0.1.1", "0.1.0").unwrap());
        assert!(!newer("0.1.0", "0.1.1").unwrap());
        assert!(!newer("0.1.0", "0.1.0").unwrap());
        assert!(newer("1.0.0", "1.0.0-rc.1").unwrap());
        assert!(newer("1.0.0-rc.10", "1.0.0-rc.2").unwrap());
        assert!(!newer("1.0.0+other", "1.0.0+build").unwrap());
        for invalid in ["../escape", "1.2", "01.2.3", "1.2.3.4", "1.2.3-", "1.2.3 "] {
            assert!(!valid_version(invalid), "{invalid}");
        }
    }

    #[test]
    fn release_rejects_drafts_prereleases_missing_assets_and_foreign_urls() {
        assert!(parse_release(&release_data()).is_ok());
        let mut data = release_data();
        data["draft"] = true.into();
        assert!(parse_release(&data).is_err());
        let mut data = release_data();
        data["prerelease"] = true.into();
        assert!(parse_release(&data).is_err());
        let mut data = release_data();
        data["tag_name"] = "v../escape".into();
        assert!(parse_release(&data).is_err());
        let mut data = release_data();
        data["assets"] = serde_json::json!([]);
        assert!(parse_release(&data).is_err());
        let mut data = release_data();
        data["assets"][0]["browser_download_url"] = "https://evil.invalid/cargoc.zip".into();
        assert!(parse_release(&data).is_err());
        let mut data = release_data();
        let duplicate = data["assets"][0].clone();
        data["assets"].as_array_mut().unwrap().push(duplicate);
        assert!(parse_release(&data).is_err());
    }

    fn make_archive(path: &Path, member: &str) {
        let payload = b"test binary contents";
        #[cfg(windows)]
        {
            let mut writer = zip::ZipWriter::new(File::create(path).unwrap());
            writer
                .start_file(member, zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.write_all(payload).unwrap();
            writer.finish().unwrap();
        }
        #[cfg(not(windows))]
        {
            let writer = flate2::write::GzEncoder::new(
                File::create(path).unwrap(),
                flate2::Compression::default(),
            );
            let mut writer = tar::Builder::new(writer);
            let mut header = tar::Header::new_gnu();
            header.set_size(payload.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            writer
                .append_data(&mut header, member, &payload[..])
                .unwrap();
            writer.into_inner().unwrap().finish().unwrap();
        }
    }

    #[test]
    fn native_archive_extracts_only_expected_binary_and_never_overwrites() {
        let directory = tempfile::tempdir().unwrap();
        let release = parse_release(&release_data()).unwrap();
        let archive = directory.path().join(&release.archive);
        let destination = directory.path().join(binary_name());
        let root = format!("cargoc-v1.2.3-{}", target().unwrap());
        make_archive(&archive, &format!("{root}/{}", binary_name()));
        extract_binary(&archive, &release, &destination).unwrap();
        assert_eq!(fs::read(&destination).unwrap(), b"test binary contents");
        assert!(extract_binary(&archive, &release, &destination).is_err());
        fs::remove_file(&destination).unwrap();
        make_archive(&archive, &format!("wrong-root/{}", binary_name()));
        assert!(extract_binary(&archive, &release, &destination).is_err());
        assert!(!destination.exists());
    }

    #[test]
    fn checksum_requires_the_exact_archive() {
        let digest = "a".repeat(64);
        assert!(
            checksum(
                format!("{digest}  cargoc-v1.0.0-x.zip\n").as_bytes(),
                "cargoc-v1.0.0-x.zip"
            )
            .is_ok()
        );
        assert!(
            checksum(
                format!("{digest}  other.zip\n").as_bytes(),
                "cargoc-v1.0.0-x.zip"
            )
            .is_err()
        );
        assert!(
            checksum(
                format!("{}  cargoc-v1.0.0-x.zip\n", "g".repeat(64)).as_bytes(),
                "cargoc-v1.0.0-x.zip"
            )
            .is_err()
        );
    }
    #[test]
    fn archive_members_cannot_escape_the_expected_root() {
        let binary = if cfg!(windows) {
            "cargoc.exe"
        } else {
            "cargoc"
        };
        assert!(safe_member(
            Path::new(&format!("cargoc-v1.0.0-x/{binary}")),
            "cargoc-v1.0.0-x"
        ));
        assert!(!safe_member(
            Path::new("../cargoc-v1.0.0-x/cargoc"),
            "cargoc-v1.0.0-x"
        ));
        assert!(!safe_member(
            Path::new("cargoc-v1.0.0-x/../cargoc"),
            "cargoc-v1.0.0-x"
        ));
        assert!(!safe_member(
            Path::new("cargoc-v1.0.0-x/cargoc/extra"),
            "cargoc-v1.0.0-x"
        ));
        assert!(valid_version("1.2.3-rc.1"));
        assert!(valid_version("1.2.3+build.7"));
        assert!(!valid_version("../escape"));
        assert!(!valid_version("1.2"));
    }
}
