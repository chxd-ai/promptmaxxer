//! API keys in the macOS login keychain (service `promptmaxxer`, account per provider: `anthropic`,
//! `custom-api`), or on Windows in Credential Manager (generic credential `promptmaxxer:<account>`, user name
//! `<account>`, stored for this user on this PC). Keys are never logged or put in error text.

use crate::EngineError;
#[cfg(target_os = "macos")]
use security_framework::passwords::{delete_generic_password, get_generic_password, set_generic_password};

pub const SERVICE: &str = "promptmaxxer";

pub type Result<T> = std::result::Result<T, EngineError>;

/// The saved secret, or None when there is none (or the keychain refused).
pub fn get(account: &str) -> Option<String> {
    get_in(SERVICE, account)
}

pub fn set(account: &str, secret: &str) -> Result<()> {
    set_in(SERVICE, account, secret)
}

/// Deleting a missing item is not an error.
pub fn delete(account: &str) -> Result<()> {
    delete_in(SERVICE, account)
}

pub(crate) fn get_in(service: &str, account: &str) -> Option<String> {
    let bytes = backend_get(service, account)?;
    let s = String::from_utf8(bytes).ok()?;
    let s = s.trim().to_string();
    (!s.is_empty()).then_some(s)
}

pub(crate) fn set_in(service: &str, account: &str, secret: &str) -> Result<()> {
    let secret = secret.trim();
    if secret.is_empty() {
        return Err(EngineError::Keychain("empty secret".into()));
    }
    backend_set(service, account, secret)
}

#[cfg(target_os = "macos")]
fn backend_get(service: &str, account: &str) -> Option<Vec<u8>> {
    get_generic_password(service, account).ok()
}

#[cfg(target_os = "macos")]
fn backend_set(service: &str, account: &str, secret: &str) -> Result<()> {
    set_generic_password(service, account, secret.as_bytes())
        .map_err(|e| EngineError::Keychain(format!("could not save to the keychain (code {})", e.code())))
}

#[cfg(target_os = "macos")]
pub(crate) fn delete_in(service: &str, account: &str) -> Result<()> {
    match delete_generic_password(service, account) {
        Ok(()) => Ok(()),
        Err(e) if e.code() == -25300 => Ok(()), // errSecItemNotFound
        Err(e) => Err(EngineError::Keychain(format!("could not delete from the keychain (code {})", e.code()))),
    }
}

#[cfg(windows)]
mod wincred {
    use windows_sys::Win32::Foundation::{GetLastError, ERROR_NOT_FOUND};
    use windows_sys::Win32::Security::Credentials::{
        CredDeleteW, CredFree, CredReadW, CredWriteW, CREDENTIALW, CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC,
    };

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// Credential Manager target name: one entry per service and account.
    fn target(service: &str, account: &str) -> Vec<u16> {
        wide(&format!("{service}:{account}"))
    }

    pub fn get(service: &str, account: &str) -> Option<Vec<u8>> {
        let t = target(service, account);
        let mut cred: *mut CREDENTIALW = std::ptr::null_mut();
        // SAFETY: `t` is NUL-terminated; on success `cred` points to a block we copy from and free once.
        unsafe {
            if CredReadW(t.as_ptr(), CRED_TYPE_GENERIC, 0, &mut cred) == 0 || cred.is_null() {
                return None;
            }
            let c = &*cred;
            let bytes = if c.CredentialBlob.is_null() || c.CredentialBlobSize == 0 {
                Vec::new()
            } else {
                std::slice::from_raw_parts(c.CredentialBlob, c.CredentialBlobSize as usize).to_vec()
            };
            CredFree(cred as *const _);
            Some(bytes)
        }
    }

    /// Ok or the Windows error code (never the secret).
    pub fn set(service: &str, account: &str, secret: &str) -> Result<(), u32> {
        let mut t = target(service, account);
        let mut user = wide(account);
        let mut blob = secret.as_bytes().to_vec();
        // SAFETY: every pointer stays valid for the call; CredWriteW copies the data.
        unsafe {
            let mut c: CREDENTIALW = std::mem::zeroed();
            c.Type = CRED_TYPE_GENERIC;
            c.TargetName = t.as_mut_ptr();
            c.UserName = user.as_mut_ptr();
            c.CredentialBlobSize = blob.len() as u32;
            c.CredentialBlob = blob.as_mut_ptr();
            c.Persist = CRED_PERSIST_LOCAL_MACHINE;
            if CredWriteW(&c, 0) == 0 {
                return Err(GetLastError());
            }
        }
        Ok(())
    }

    /// Ok when deleted or not there.
    pub fn delete(service: &str, account: &str) -> Result<(), u32> {
        let t = target(service, account);
        // SAFETY: `t` is NUL-terminated.
        unsafe {
            if CredDeleteW(t.as_ptr(), CRED_TYPE_GENERIC, 0) == 0 {
                let e = GetLastError();
                if e != ERROR_NOT_FOUND {
                    return Err(e);
                }
            }
        }
        Ok(())
    }
}

#[cfg(windows)]
fn backend_get(service: &str, account: &str) -> Option<Vec<u8>> {
    wincred::get(service, account)
}

#[cfg(windows)]
fn backend_set(service: &str, account: &str, secret: &str) -> Result<()> {
    wincred::set(service, account, secret)
        .map_err(|code| EngineError::Keychain(format!("could not save to Windows Credential Manager (code {code})")))
}

#[cfg(windows)]
pub(crate) fn delete_in(service: &str, account: &str) -> Result<()> {
    wincred::delete(service, account)
        .map_err(|code| EngineError::Keychain(format!("could not delete from Windows Credential Manager (code {code})")))
}

// Other systems: no secret store, so no saved keys (API keys can still come from the run options).
#[cfg(not(any(target_os = "macos", windows)))]
fn backend_get(_service: &str, _account: &str) -> Option<Vec<u8>> {
    None
}

#[cfg(not(any(target_os = "macos", windows)))]
fn backend_set(_service: &str, _account: &str, _secret: &str) -> Result<()> {
    Err(EngineError::Keychain("no secret store on this system".into()))
}

#[cfg(not(any(target_os = "macos", windows)))]
pub(crate) fn delete_in(_service: &str, _account: &str) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_item_reads_as_none_and_deletes_cleanly() {
        // Read and delete of an item that does not exist: no keychain change.
        let acct = format!("pmx-test-missing-{}", std::process::id());
        assert_eq!(get_in("promptmaxxer-test", &acct), None);
        assert!(delete_in("promptmaxxer-test", &acct).is_ok());
    }

    #[test]
    fn empty_secret_is_refused_before_the_keychain() {
        assert!(set_in("promptmaxxer-test", "x", "   ").is_err());
    }

    /// Writes a real keychain item and removes it. Run by hand: `cargo test -- --ignored keychain_round_trip`.
    #[test]
    #[ignore]
    fn keychain_round_trip() {
        let acct = format!("pmx-test-{}", std::process::id());
        set_in("promptmaxxer-test", &acct, "not-a-real-key").unwrap();
        assert_eq!(get_in("promptmaxxer-test", &acct).as_deref(), Some("not-a-real-key"));
        delete_in("promptmaxxer-test", &acct).unwrap();
        assert_eq!(get_in("promptmaxxer-test", &acct), None);
    }
}
