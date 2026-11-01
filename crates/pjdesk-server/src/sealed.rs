//! Seals server secrets (the cloud sign-in token, R2 keys, the backup
//! encryption key, the network signing key) before they are written to disk.
//!
//! On Windows this is machine-scope DPAPI, so the SYSTEM service and the
//! management window can both open it, while a copy of the file taken to
//! another computer cannot. The file ACLs set by `cloud::protect` still apply.
//! Values written by older builds are plain text and still open; the next save
//! seals them. Elsewhere (development builds) values stay as written.

const TAG: &str = "dpapi-machine:";

/// Seals `secret` for this machine. Empty stays empty. Already sealed text is left alone.
pub fn seal(secret: &str) -> String {
    if secret.is_empty() || secret.starts_with(TAG) {
        return secret.to_string();
    }
    #[cfg(windows)]
    {
        if let Some(bytes) = dpapi::protect(secret.as_bytes()) {
            return format!("{TAG}{}", hex::encode(bytes));
        }
    }
    secret.to_string()
}

/// Opens a value written by [`seal`], or returns plain text from an older build as is.
/// A sealed value that cannot be opened here comes back empty.
pub fn open(stored: &str) -> String {
    let Some(hexed) = stored.strip_prefix(TAG) else {
        return stored.to_string();
    };
    #[cfg(windows)]
    {
        if let Some(text) = hex::decode(hexed).ok().and_then(|b| dpapi::unprotect(&b)).and_then(|b| String::from_utf8(b).ok()) {
            return text;
        }
    }
    let _ = hexed;
    String::new()
}

/// True when a non-empty value on disk is still plain text and could be sealed here.
pub fn needs_seal(stored: &str) -> bool {
    cfg!(windows) && !stored.is_empty() && !stored.starts_with(TAG)
}

#[cfg(windows)]
mod dpapi {
    use windows::Win32::Foundation::{LocalFree, HLOCAL};
    use windows::Win32::Security::Cryptography::{CryptProtectData, CryptUnprotectData, CRYPT_INTEGER_BLOB};

    /// CRYPTPROTECT_UI_FORBIDDEN | CRYPTPROTECT_LOCAL_MACHINE
    const FLAGS: u32 = 0x1 | 0x4;

    fn as_blob(data: &[u8]) -> CRYPT_INTEGER_BLOB {
        CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr() as *mut u8 }
    }

    unsafe fn take(blob: CRYPT_INTEGER_BLOB) -> Vec<u8> {
        let out = std::slice::from_raw_parts(blob.pbData, blob.cbData as usize).to_vec();
        let _ = LocalFree(Some(HLOCAL(blob.pbData as *mut _)));
        out
    }

    pub fn protect(data: &[u8]) -> Option<Vec<u8>> {
        unsafe {
            let input = as_blob(data);
            let mut output = CRYPT_INTEGER_BLOB::default();
            CryptProtectData(&input, None, None, None, None, FLAGS, &mut output).ok()?;
            Some(take(output))
        }
    }

    pub fn unprotect(data: &[u8]) -> Option<Vec<u8>> {
        unsafe {
            let input = as_blob(data);
            let mut output = CRYPT_INTEGER_BLOB::default();
            CryptUnprotectData(&input, None, None, None, None, 0x1, &mut output).ok()?;
            Some(take(output))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_from_older_builds_still_opens_and_round_trips() {
        assert_eq!(open("abc"), "abc");
        assert_eq!(open(""), "");
        assert_eq!(seal(""), "");
        assert_eq!(open(&seal("k3y")), "k3y");
        assert_eq!(open(&seal(&seal("k3y"))), "k3y");
        assert_eq!(open("dpapi-machine:zz"), "");
    }
}
