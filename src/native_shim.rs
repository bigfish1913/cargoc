//! Single-binary native proxy. The fixed-size trailer is an ownership/integrity
//! marker, not an authenticity signature. Never infer ownership from an EXE name.
use crate::{
    Result, fail,
    util::{absolute, check_path, identity_path},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

const BLOCK: usize = 32 * 1024;
const MAGIC: &[u8] = b"\0CARGOC-NATIVE-PROXY-v1\0";
const DIGEST: usize = BLOCK - 32;

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Metadata {
    pub target: PathBuf,
    pub config: PathBuf,
}

fn corrupt<T>() -> Result<T> {
    fail("Corrupt cargoc native proxy trailer; refusing to use or replace it")
}

fn hash_prefix(file: &mut File, length: u64) -> Result<Vec<u8>> {
    file.rewind()?;
    let mut hasher = Sha256::new();
    let mut remaining = length;
    let mut buffer = [0_u8; 64 * 1024];
    while remaining > 0 {
        let count = file.read(&mut buffer[..remaining.min(64 * 1024) as usize])?;
        if count == 0 {
            return corrupt();
        }
        hasher.update(&buffer[..count]);
        remaining -= count as u64;
    }
    Ok(hasher.finalize().to_vec())
}

/// Reads at most one bounded trailer into memory, hashing the image in chunks.
/// Invalid marked files are errors (not owned files safe to overwrite/delete).
/// Target liveness is deliberately separate so a stale proxy can be refreshed.
pub fn inspect(path: &Path) -> Result<Option<Metadata>> {
    let mut file = File::open(path)?;
    let length = file.metadata()?.len();
    let tail_len = length.min((BLOCK + MAGIC.len()) as u64) as usize;
    file.seek(SeekFrom::End(-(tail_len as i64)))?;
    let mut tail = vec![0; tail_len];
    file.read_exact(&mut tail)?;
    let block = tail.get(tail_len.saturating_sub(BLOCK)..).unwrap_or(&[]);
    if block.len() != BLOCK || !block.starts_with(MAGIC) {
        if tail.windows(MAGIC.len()).any(|part| part == MAGIC)
            || (8..MAGIC.len()).any(|size| tail.ends_with(&MAGIC[..size]))
        {
            return corrupt();
        }
        return Ok(None);
    }
    let start = MAGIC.len();
    let size = u32::from_le_bytes(block[start..start + 4].try_into()?) as usize;
    let payload = start + 4;
    if size == 0 || size > DIGEST - payload || block[payload + size..DIGEST].iter().any(|b| *b != 0)
    {
        return corrupt();
    }
    if hash_prefix(&mut file, length - 32)? != block[DIGEST..] {
        return corrupt();
    }
    let metadata: Metadata = match serde_json::from_slice(&block[payload..payload + size]) {
        Ok(value) => value,
        Err(_) => return corrupt(),
    };
    if !metadata.target.is_absolute() || !metadata.config.is_absolute() {
        return corrupt();
    }
    Ok(Some(metadata))
}

fn validate_target(proxy: &Path, target: &Path) -> Result<()> {
    if identity_path(proxy)? == identity_path(target)? {
        return fail("Native proxy recursion: target points to itself");
    }
    if !target.is_file() {
        return fail(format!(
            "Native proxy target is missing: {}",
            target.display()
        ));
    }
    if inspect(target)?.is_some() {
        return fail("Native proxy recursion: target is another native proxy");
    }
    Ok(())
}

/// Install/refresh only a verified owned proxy; unknown/corrupt files survive.
pub fn install(proxy: &Path, target: &Path, config: &Path) -> Result<()> {
    check_path(proxy)?;
    if proxy.exists() && inspect(proxy)?.is_none() {
        return fail(format!(
            "Refusing to replace an unowned shim: {}",
            proxy.display()
        ));
    }
    let target = absolute(target)?;
    validate_target(proxy, &target)?;
    let metadata = serde_json::to_vec(&Metadata {
        target: target.clone(),
        config: absolute(config)?,
    })?;
    let payload = MAGIC.len() + 4;
    if metadata.len() > DIGEST - payload {
        return fail("Native proxy metadata exceeds size limit");
    }
    let parent = proxy.parent().ok_or("Proxy has no parent directory")?;
    fs::create_dir_all(parent)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    std::io::copy(&mut File::open(target)?, temporary.as_file_mut())?;
    let mut block = vec![0; BLOCK];
    block[..MAGIC.len()].copy_from_slice(MAGIC);
    block[MAGIC.len()..payload].copy_from_slice(&(metadata.len() as u32).to_le_bytes());
    block[payload..payload + metadata.len()].copy_from_slice(&metadata);
    temporary.write_all(&block[..DIGEST])?;
    let length = temporary.as_file().metadata()?.len();
    let digest = hash_prefix(temporary.as_file_mut(), length)?;
    temporary.seek(SeekFrom::End(0))?;
    temporary.write_all(&digest)?;
    temporary.as_file().sync_all()?;
    temporary.persist(proxy).map_err(|error| error.error)?;
    Ok(())
}

/// Called before Clap: no shell, no UTF-8 argument conversion, inherited stdio.
#[cfg(windows)]
pub fn dispatch() -> Result<Option<i32>> {
    let executable = std::env::current_exe()?;
    let Some(metadata) = inspect(&executable)? else {
        // A stripped/truncated proxy must not silently become a normal CLI.
        if executable
            .file_name()
            .is_some_and(|n| n.eq_ignore_ascii_case("cargo.exe"))
        {
            return fail("Unowned or corrupt cargo.exe native proxy");
        }
        return Ok(None);
    };
    validate_target(&executable, &metadata.target)?;
    let status = crate::cargo::command(&metadata.target)?
        .arg("--cargoc-config")
        .arg(metadata.config)
        .arg("exec")
        .arg("--")
        .args(std::env::args_os().skip(1))
        .status()?;
    Ok(Some(status.code().unwrap_or(1)))
}

#[cfg(not(windows))]
pub fn dispatch() -> Result<Option<i32>> {
    Ok(None)
}
