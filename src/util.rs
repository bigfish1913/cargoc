use crate::{Result, fail};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Component, Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

pub const GIB: f64 = 1_073_741_824.0;

pub fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

pub fn absolute(path: &Path) -> Result<PathBuf> {
    let full = std::path::absolute(path)?;
    let mut normalized = PathBuf::new();
    for component in full.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            value => normalized.push(value.as_os_str()),
        }
    }
    Ok(normalized)
}

/// Stable textual identity; Windows drive-case and verbatim prefixes are aliases.
pub fn identity_path(path: &Path) -> Result<String> {
    let path = absolute(path)?;
    let path = match fs::canonicalize(&path) {
        Ok(resolved) => resolved,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => path,
        Err(error) => return Err(error.into()),
    };
    let value = path
        .to_str()
        .ok_or("Workspace/cache paths must be valid UTF-8")?
        .to_owned();
    #[cfg(windows)]
    {
        let value = value.replace('\\', "/").to_lowercase();
        if let Some(rest) = value.strip_prefix("//?/unc/") {
            return Ok(format!("//{rest}"));
        }
        Ok(value.strip_prefix("//?/").unwrap_or(&value).to_owned())
    }
    #[cfg(not(windows))]
    Ok(value)
}

pub fn same_or_descendant(path: &Path, parent: &Path) -> Result<bool> {
    let path = identity_path(path)?;
    let parent = identity_path(parent)?;
    let parent = parent.trim_end_matches('/');
    Ok(path == parent || path.starts_with(&format!("{parent}/")))
}

/// Refuse symlinks and Windows reparse points in every existing component.
pub fn check_path(path: &Path) -> Result<()> {
    let path = absolute(path)?;
    for part in path.ancestors() {
        match fs::symlink_metadata(part) {
            Ok(meta) => {
                #[cfg(windows)]
                let reparse = {
                    use std::os::windows::fs::MetadataExt;
                    meta.file_attributes() & 0x400 != 0
                };
                #[cfg(not(windows))]
                let reparse = false;
                if meta.file_type().is_symlink() || reparse {
                    return fail(format!(
                        "Refusing symlink/reparse point: {}",
                        part.display()
                    ));
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

pub fn atomic_write(path: &Path, content: &[u8]) -> Result<()> {
    check_path(path)?;
    let parent = path.parent().ok_or("File has no parent")?;
    fs::create_dir_all(parent)?;
    let temp = parent.join(format!(
        ".cargoc-{}-{}.tmp",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
    ));
    let result = (|| -> Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        file.write_all(content)?;
        file.sync_all()?;
        drop(file);
        // Windows rename does not replace an existing file. ReplaceFileW provides
        // atomic replacement; new files can use rename directly.
        #[cfg(windows)]
        if path.exists() {
            use std::os::windows::ffi::OsStrExt;
            use windows_sys::Win32::Storage::FileSystem::ReplaceFileW;
            let dst: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
            let src: Vec<u16> = temp.as_os_str().encode_wide().chain(Some(0)).collect();
            // SAFETY: Both pointers reference live, NUL-terminated UTF-16 strings.
            #[allow(unsafe_code)]
            if unsafe {
                ReplaceFileW(
                    dst.as_ptr(),
                    src.as_ptr(),
                    std::ptr::null(),
                    0,
                    std::ptr::null(),
                    std::ptr::null(),
                )
            } == 0
            {
                return Err(std::io::Error::last_os_error().into());
            }
            return Ok(());
        }
        fs::rename(&temp, path)?;
        Ok(())
    })();
    let _ = fs::remove_file(&temp);
    result
}
