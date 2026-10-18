//! Encrypt small secrets (saved passwords, login tokens) at rest.
//!
//! On Windows this uses DPAPI (`CryptProtectData`), so the ciphertext can only
//! be decrypted by the same Windows user (or, for the service, the same machine
//! account) on the same machine. Elsewhere — and if DPAPI ever fails — it falls
//! back to a reversible encoding that is clearly marked as unprotected, so the
//! product still works for development and testing without pretending the value
//! is encrypted.
//!
//! Stored form is a tagged, base64-ish hex string so it round-trips through
//! JSON and is easy to tell apart from plain text.

/// Encrypts `secret` into a string safe to store in JSON.
///
/// On Windows the blob is bound to the current user. The background service
/// uses [`protect_machine`] instead, so the same secret can be read at the
/// sign-in screen, where the helper runs as SYSTEM.
pub fn protect(secret: &str) -> String {
    protect_inner(secret, false)
}

/// Like [`protect`], but on Windows any process on this machine can decrypt
/// it. The file that holds the result must itself be locked to SYSTEM, Administrators
/// and the signed-in user — local-machine DPAPI is not a substitute for an ACL.
pub fn protect_machine(secret: &str) -> String {
    protect_inner(secret, true)
}

fn protect_inner(secret: &str, machine: bool) -> String {
    #[cfg(windows)]
    {
        if let Some(bytes) = dpapi::protect(secret.as_bytes(), machine) {
            let tag = if machine { "dpapi-machine" } else { "dpapi" };
            return format!("{tag}:{}", hex::encode(bytes));
        }
    }
    let _ = machine;
    // Fallback: clearly not encrypted.
    format!("plain:{}", hex::encode(secret.as_bytes()))
}

/// Reverses [`protect`] or [`protect_machine`]. Returns `None` if the value
/// can't be decoded.
pub fn reveal(stored: &str) -> Option<String> {
    if let Some(hexed) = stored.strip_prefix("plain:") {
        return hex::decode(hexed).ok().and_then(|b| String::from_utf8(b).ok());
    }
    #[cfg(windows)]
    if let Some(hexed) = stored.strip_prefix("dpapi-machine:").or_else(|| stored.strip_prefix("dpapi:")) {
        let bytes = hex::decode(hexed).ok()?;
        return dpapi::unprotect(&bytes).and_then(|b| String::from_utf8(b).ok());
    }
    // Unknown/foreign tag (e.g. a dpapi blob on a non-Windows build): give up.
    let _ = stored;
    None
}

#[cfg(windows)]
mod dpapi {
    use windows::Win32::Foundation::{LocalFree, HLOCAL};
    use windows::Win32::Security::Cryptography::{CryptProtectData, CryptUnprotectData, CRYPT_INTEGER_BLOB};

    fn as_blob(data: &[u8]) -> CRYPT_INTEGER_BLOB {
        CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr() as *mut u8 }
    }

    /// Reads an output blob into a Vec and frees the OS allocation.
    unsafe fn take(blob: CRYPT_INTEGER_BLOB) -> Vec<u8> {
        let out = std::slice::from_raw_parts(blob.pbData, blob.cbData as usize).to_vec();
        let _ = LocalFree(Some(HLOCAL(blob.pbData as *mut _)));
        out
    }

    pub fn protect(data: &[u8], machine: bool) -> Option<Vec<u8>> {
        unsafe {
            let input = as_blob(data);
            let mut output = CRYPT_INTEGER_BLOB::default();
            // CRYPTPROTECT_UI_FORBIDDEN (0x1): never show UI (we may be a service).
            // CRYPTPROTECT_LOCAL_MACHINE (0x4): decryptable by SYSTEM at the
            // sign-in screen, not only by the user who set the password.
            let flags = if machine { 0x1 | 0x4 } else { 0x1 };
            CryptProtectData(&input, None, None, None, None, flags, &mut output).ok()?;
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
    fn roundtrip() {
        let secret = "hunter2 🔐";
        let stored = protect(secret);
        assert_ne!(stored, secret);
        assert_eq!(reveal(&stored).as_deref(), Some(secret));
    }

    #[test]
    fn plain_fallback_is_marked() {
        // Whatever the platform, a plain-tagged value must decode.
        assert_eq!(reveal(&format!("plain:{}", hex::encode("abc"))).as_deref(), Some("abc"));
    }
}
