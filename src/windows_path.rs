//! Raw UTF-16 registry access: preserve non-ASCII text and REG_EXPAND_SZ exactly.
use crate::{Result, fail};
use windows_sys::{
    Win32::{
        Foundation::{ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, ERROR_SUCCESS},
        System::Registry::{
            HKEY_CURRENT_USER, REG_EXPAND_SZ, REG_SZ, RRF_NOEXPAND, RRF_RT_REG_EXPAND_SZ,
            RRF_RT_REG_SZ, RegGetValueW, RegSetKeyValueW,
        },
        UI::WindowsAndMessaging::{
            HWND_BROADCAST, SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_SETTINGCHANGE,
        },
    },
    core::w,
};

pub fn read() -> Result<Option<(String, String)>> {
    for _ in 0..4 {
        let mut bytes = 0_u32;
        let mut kind = 0_u32;
        let flags = RRF_NOEXPAND | RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ;
        // SAFETY: Static NUL-terminated names, valid output pointers, no data buffer.
        #[allow(unsafe_code)]
        let status = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                w!("Environment"),
                w!("Path"),
                flags,
                &mut kind,
                std::ptr::null_mut(),
                &mut bytes,
            )
        };
        if status == ERROR_FILE_NOT_FOUND {
            return Ok(None);
        }
        if status != ERROR_SUCCESS {
            return Err(std::io::Error::from_raw_os_error(status as i32).into());
        }
        if !bytes.is_multiple_of(2) {
            return fail("Malformed UTF-16 user PATH; refusing to modify it");
        }
        let mut data = vec![0_u16; bytes as usize / 2 + 1];
        let mut capacity = u32::try_from(data.len() * 2)?;
        // SAFETY: The writable buffer is capacity bytes, and all pointers stay live.
        #[allow(unsafe_code)]
        let status = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                w!("Environment"),
                w!("Path"),
                flags,
                &mut kind,
                data.as_mut_ptr().cast(),
                &mut capacity,
            )
        };
        if status == ERROR_MORE_DATA {
            continue;
        }
        if status != ERROR_SUCCESS {
            return Err(std::io::Error::from_raw_os_error(status as i32).into());
        }
        if !capacity.is_multiple_of(2) {
            return fail("Malformed UTF-16 user PATH");
        }
        data.truncate(capacity as usize / 2);
        while data.last() == Some(&0) {
            data.pop();
        }
        if data.contains(&0) {
            return fail("Embedded NUL in user PATH; refusing to modify it");
        }
        let value = String::from_utf16(&data)?;
        let kind = match kind {
            REG_SZ => "REG_SZ",
            REG_EXPAND_SZ => "REG_EXPAND_SZ",
            _ => return fail("Unsupported user PATH registry type"),
        };
        return Ok(Some((kind.to_owned(), value)));
    }
    fail("User PATH changed while reading; retry setup")
}

pub fn write(value: &str, kind: &str) -> Result<()> {
    if value.contains('\0') {
        return fail("NUL in user PATH");
    }
    let kind = match kind {
        "REG_SZ" => REG_SZ,
        "REG_EXPAND_SZ" => REG_EXPAND_SZ,
        _ => return fail("Unsupported user PATH registry type"),
    };
    let value: Vec<u16> = value.encode_utf16().chain(Some(0)).collect();
    let bytes = u32::try_from(value.len() * 2)?;
    // SAFETY: All strings are NUL-terminated; buffer length includes its terminator.
    #[allow(unsafe_code)]
    let status = unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            w!("Environment"),
            w!("Path"),
            kind,
            value.as_ptr().cast(),
            bytes,
        )
    };
    if status != ERROR_SUCCESS {
        return Err(std::io::Error::from_raw_os_error(status as i32).into());
    }
    // Notify Explorer so terminals launched from it receive the updated environment.
    // SAFETY: The static message string outlives this synchronous timeout call.
    #[allow(unsafe_code)]
    let notified = unsafe {
        SendMessageTimeoutW(
            HWND_BROADCAST,
            WM_SETTINGCHANGE,
            0,
            w!("Environment") as isize,
            SMTO_ABORTIFHUNG,
            1000,
            std::ptr::null_mut(),
        )
    };
    if notified == 0 {
        eprintln!(
            "cargoc: user PATH saved; notification timed out. Restart/sign in again if new terminals still use the old PATH."
        );
    }
    Ok(())
}
