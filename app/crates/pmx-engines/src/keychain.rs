//! API keys in the macOS login keychain: service `promptmaxxer`, account per provider (`anthropic`).
//! Keys are never logged or put in error text.

use crate::EngineError;
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
    let bytes = get_generic_password(service, account).ok()?;
    let s = String::from_utf8(bytes).ok()?;
    let s = s.trim().to_string();
    (!s.is_empty()).then_some(s)
}

pub(crate) fn set_in(service: &str, account: &str, secret: &str) -> Result<()> {
    let secret = secret.trim();
    if secret.is_empty() {
        return Err(EngineError::Keychain("empty secret".into()));
    }
    set_generic_password(service, account, secret.as_bytes())
        .map_err(|e| EngineError::Keychain(format!("could not save to the keychain (code {})", e.code())))
}

pub(crate) fn delete_in(service: &str, account: &str) -> Result<()> {
    match delete_generic_password(service, account) {
        Ok(()) => Ok(()),
        Err(e) if e.code() == -25300 => Ok(()), // errSecItemNotFound
        Err(e) => Err(EngineError::Keychain(format!("could not delete from the keychain (code {})", e.code()))),
    }
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
