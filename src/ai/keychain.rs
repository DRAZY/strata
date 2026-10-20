//! API keys in the system's own store: the macOS Keychain (a generic
//! password per provider origin, service "Strata AI") or the Windows
//! Credential Manager (a generic credential "Strata AI/<origin>"). Never in
//! settings.json, never logged. Other systems have no store Strata can rely
//! on, so keys can't be saved there (local providers and command-line tools
//! need none).

pub const SERVICE: &str = "Strata AI";

pub trait KeyStore: Send + Sync {
    fn get(&self, account: &str) -> Result<Option<String>, String>;
    fn set(&self, account: &str, key: &str) -> Result<(), String>;
    /// Removing a key that isn't there is fine.
    fn delete(&self, account: &str) -> Result<(), String>;
    /// Whether a key is saved, without reading it (no prompt on macOS).
    fn exists(&self, account: &str) -> Result<bool, String>;
}

/// The system's store.
pub struct System;

/// Keys in memory: tests, and nothing else.
#[cfg(test)]
#[derive(Default)]
pub struct Memory(std::sync::Mutex<std::collections::HashMap<String, String>>);

#[cfg(test)]
impl KeyStore for Memory {
    fn get(&self, account: &str) -> Result<Option<String>, String> {
        Ok(self.0.lock().unwrap_or_else(|e| e.into_inner()).get(account).cloned())
    }
    fn set(&self, account: &str, key: &str) -> Result<(), String> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).insert(account.to_owned(), key.to_owned());
        Ok(())
    }
    fn delete(&self, account: &str) -> Result<(), String> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).remove(account);
        Ok(())
    }
    fn exists(&self, account: &str) -> Result<bool, String> {
        Ok(self.0.lock().unwrap_or_else(|e| e.into_inner()).contains_key(account))
    }
}

/// What the store is called, for messages.
pub fn store_name() -> &'static str {
    if cfg!(target_os = "macos") {
        "the Keychain"
    } else if cfg!(windows) {
        "Windows Credential Manager"
    } else {
        "the system keychain"
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use objc2_core_foundation::{CFBoolean, CFData, CFDictionary, CFRetained, CFString, CFType};
    use objc2_security::{
        SecItemAdd, SecItemCopyMatching, SecItemDelete, SecItemUpdate, errSecItemNotFound, errSecSuccess, kSecAttrAccount, kSecAttrLabel, kSecAttrService, kSecClass, kSecClassGenericPassword, kSecMatchLimit, kSecMatchLimitOne, kSecReturnData,
        kSecValueData,
    };
    use std::ptr::NonNull;

    fn status(what: &str, code: i32) -> String {
        format!("{what} failed (Keychain error {code})")
    }

    /// The item's query plus `extra` pairs.
    fn query(account: &str, extra: &[(&CFString, &CFType)]) -> CFRetained<CFDictionary<CFString, CFType>> {
        let service = CFString::from_str(super::SERVICE);
        let account = CFString::from_str(account);
        // SAFETY: the kSec* keys are constant CFStrings the Security framework exports.
        let (class, class_value, k_service, k_account) = unsafe { (kSecClass, kSecClassGenericPassword, kSecAttrService, kSecAttrAccount) };
        let mut keys: Vec<&CFString> = vec![class, k_service, k_account];
        let mut values: Vec<&CFType> = vec![class_value.as_ref(), service.as_ref(), account.as_ref()];
        for (k, v) in extra {
            keys.push(k);
            values.push(v);
        }
        CFDictionary::from_slices(&keys, &values)
    }

    pub fn get(account: &str) -> Result<Option<String>, String> {
        // SAFETY: constant CFStrings and CFBoolean from the frameworks.
        let (ret, limit, one) = unsafe { (kSecReturnData, kSecMatchLimit, kSecMatchLimitOne) };
        let q = query(account, &[(ret, CFBoolean::new(true).as_ref()), (limit, one.as_ref())]);
        let mut out: *const CFType = std::ptr::null();
        // SAFETY: `q` is a valid dictionary of CF keys and values; `out` is a valid out pointer.
        let code = unsafe { SecItemCopyMatching(q.as_opaque(), &mut out) };
        if code == errSecItemNotFound {
            return Ok(None);
        }
        if code != errSecSuccess {
            return Err(status("Reading the key", code));
        }
        let Some(ptr) = NonNull::new(out.cast_mut()) else { return Ok(None) };
        // SAFETY: SecItemCopyMatching returns the result with +1 retain count (the Create rule).
        let item: CFRetained<CFType> = unsafe { CFRetained::from_raw(ptr) };
        let data = item.downcast::<CFData>().map_err(|_| "The Keychain returned something that isn't a key".to_owned())?;
        String::from_utf8(data.to_vec()).map(Some).map_err(|_| "The saved key isn't text".to_owned())
    }

    pub fn exists(account: &str) -> Result<bool, String> {
        // SAFETY: constant CFStrings from the framework.
        let (limit, one) = unsafe { (kSecMatchLimit, kSecMatchLimitOne) };
        let q = query(account, &[(limit, one.as_ref())]);
        // SAFETY: valid dictionary; a null result pointer is allowed (nothing returned).
        let code = unsafe { SecItemCopyMatching(q.as_opaque(), std::ptr::null_mut()) };
        match code {
            c if c == errSecSuccess => Ok(true),
            c if c == errSecItemNotFound => Ok(false),
            c => Err(status("Looking for the key", c)),
        }
    }

    pub fn set(account: &str, key: &str) -> Result<(), String> {
        let data = CFData::from_bytes(key.as_bytes());
        // SAFETY: constant CFString from the framework.
        let value_key = unsafe { kSecValueData };
        let change = CFDictionary::<CFString, CFType>::from_slices(&[value_key], &[data.as_ref()]);
        let q = query(account, &[]);
        // SAFETY: both are valid dictionaries of CF keys and values.
        let code = unsafe { SecItemUpdate(q.as_opaque(), change.as_opaque()) };
        if code == errSecSuccess {
            return Ok(());
        }
        if code != errSecItemNotFound {
            return Err(status("Saving the key", code));
        }
        let label = CFString::from_str(&format!("{} key ({account})", super::SERVICE));
        // SAFETY: constant CFString from the framework.
        let label_key = unsafe { kSecAttrLabel };
        let add = query(account, &[(value_key, data.as_ref()), (label_key, label.as_ref())]);
        // SAFETY: valid dictionary; a null result pointer is allowed.
        let code = unsafe { SecItemAdd(add.as_opaque(), std::ptr::null_mut()) };
        if code == errSecSuccess { Ok(()) } else { Err(status("Saving the key", code)) }
    }

    pub fn delete(account: &str) -> Result<(), String> {
        let q = query(account, &[]);
        // SAFETY: valid dictionary.
        let code = unsafe { SecItemDelete(q.as_opaque()) };
        if code == errSecSuccess || code == errSecItemNotFound { Ok(()) } else { Err(status("Removing the key", code)) }
    }
}

#[cfg(windows)]
mod imp {
    use windows_sys::Win32::Foundation::{ERROR_NOT_FOUND, GetLastError};
    use windows_sys::Win32::Security::Credentials::{CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC, CREDENTIALW, CredDeleteW, CredFree, CredReadW, CredWriteW};

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn target(account: &str) -> Vec<u16> {
        wide(&format!("{}/{account}", super::SERVICE))
    }

    fn error(what: &str) -> String {
        // SAFETY: reads the calling thread's last error code.
        format!("{what} failed (Windows error {})", unsafe { GetLastError() })
    }

    pub fn get(account: &str) -> Result<Option<String>, String> {
        let t = target(account);
        let mut cred: *mut CREDENTIALW = std::ptr::null_mut();
        // SAFETY: `t` is a NUL-terminated UTF-16 string; `cred` is a valid out pointer.
        if unsafe { CredReadW(t.as_ptr(), CRED_TYPE_GENERIC, 0, &mut cred) } == 0 {
            // SAFETY: as above.
            if unsafe { GetLastError() } == ERROR_NOT_FOUND {
                return Ok(None);
            }
            return Err(error("Reading the key"));
        }
        // SAFETY: CredReadW succeeded, so `cred` points to a credential whose
        // blob is `CredentialBlobSize` bytes; it's freed with CredFree below.
        let bytes = unsafe {
            let c = &*cred;
            let b = if c.CredentialBlob.is_null() { Vec::new() } else { std::slice::from_raw_parts(c.CredentialBlob, c.CredentialBlobSize as usize).to_vec() };
            CredFree(cred.cast());
            b
        };
        String::from_utf8(bytes).map(Some).map_err(|_| "The saved key isn't text".to_owned())
    }

    pub fn exists(account: &str) -> Result<bool, String> {
        get(account).map(|k| k.is_some())
    }

    pub fn set(account: &str, key: &str) -> Result<(), String> {
        let mut t = target(account);
        let mut user = super::imp_user();
        let mut blob = key.as_bytes().to_vec();
        // SAFETY: an all-zero CREDENTIALW is a valid empty value (null pointers, zero sizes).
        let mut cred: CREDENTIALW = unsafe { std::mem::zeroed() };
        cred.Type = CRED_TYPE_GENERIC;
        cred.TargetName = t.as_mut_ptr();
        cred.CredentialBlobSize = u32::try_from(blob.len()).map_err(|_| "The key is too long".to_owned())?;
        cred.CredentialBlob = blob.as_mut_ptr();
        cred.Persist = CRED_PERSIST_LOCAL_MACHINE;
        cred.UserName = user.as_mut_ptr();
        // SAFETY: every pointer in `cred` points into a live buffer above.
        let ok = unsafe { CredWriteW(&cred, 0) } != 0;
        blob.fill(0);
        if ok { Ok(()) } else { Err(error("Saving the key")) }
    }

    pub fn delete(account: &str) -> Result<(), String> {
        let t = target(account);
        // SAFETY: `t` is a NUL-terminated UTF-16 string.
        if unsafe { CredDeleteW(t.as_ptr(), CRED_TYPE_GENERIC, 0) } != 0 {
            return Ok(());
        }
        // SAFETY: reads the calling thread's last error code.
        if unsafe { GetLastError() } == ERROR_NOT_FOUND { Ok(()) } else { Err(error("Removing the key")) }
    }
}

#[cfg(windows)]
fn imp_user() -> Vec<u16> {
    "Strata".encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(not(any(target_os = "macos", windows)))]
mod imp {
    fn unsupported() -> String {
        "Strata can't store API keys on this system: there's no supported keychain. Use a local provider or a command-line tool instead.".into()
    }
    pub fn get(_: &str) -> Result<Option<String>, String> {
        Ok(None)
    }
    pub fn exists(_: &str) -> Result<bool, String> {
        Ok(false)
    }
    pub fn set(_: &str, _: &str) -> Result<(), String> {
        Err(unsupported())
    }
    pub fn delete(_: &str) -> Result<(), String> {
        Ok(())
    }
}

impl KeyStore for System {
    fn get(&self, account: &str) -> Result<Option<String>, String> {
        imp::get(account)
    }
    fn set(&self, account: &str, key: &str) -> Result<(), String> {
        if !super::http::key_ok(key) {
            return Err("That doesn't look like an API key (keys have no spaces or line breaks)".into());
        }
        imp::set(account, key)
    }
    fn delete(&self, account: &str) -> Result<(), String> {
        imp::delete(account)
    }
    fn exists(&self, account: &str) -> Result<bool, String> {
        imp::exists(account)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(store: &dyn KeyStore, account: &str) {
        let _ = store.delete(account);
        assert_eq!(store.get(account).unwrap(), None);
        assert!(!store.exists(account).unwrap());
        store.set(account, "sk-test-1234").unwrap();
        assert!(store.exists(account).unwrap());
        assert_eq!(store.get(account).unwrap().as_deref(), Some("sk-test-1234"));
        // Saving again replaces it.
        store.set(account, "sk-test-5678").unwrap();
        assert_eq!(store.get(account).unwrap().as_deref(), Some("sk-test-5678"));
        store.delete(account).unwrap();
        assert_eq!(store.get(account).unwrap(), None);
        store.delete(account).unwrap();
    }

    #[test]
    fn memory_store_round_trip() {
        round_trip(&Memory::default(), "openai https://example.test");
    }

    /// The real Keychain / Credential Manager: run with STRATA_TEST_KEYCHAIN=1
    /// (it adds and removes an item named "Strata AI").
    #[test]
    fn system_store_round_trip() {
        if std::env::var_os("STRATA_TEST_KEYCHAIN").is_none() {
            return;
        }
        round_trip(&System, &format!("test {}", std::process::id()));
    }

    #[test]
    fn system_store_refuses_things_that_arent_keys() {
        for bad in ["", "has space", "line\nbreak", "tab\tkey", "ключ"] {
            assert!(System.set("test-never-saved", bad).is_err(), "{bad:?}");
        }
    }
}
