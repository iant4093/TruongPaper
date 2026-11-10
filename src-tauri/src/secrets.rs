//! Single-entry OS Keychain JSON vault (`com.iantruong.truongpaper` / `vault`, pitfall #15).
//! Spec: `docs/01-SYSTEM-ARCHITECTURE.md` §7.
//!
//! Every secret lives in ONE keychain item so macOS prompts for Keychain access at most once.
//! The [`Store`] trait keeps `cargo test` off the host keychain.

use std::collections::BTreeMap;
use std::sync::{LazyLock, Mutex};

const SERVICE: &str = "com.iantruong.truongpaper";
const ACCOUNT: &str = "vault";
const ALLOWED_KEYS: &[&str] = &["steam_username", "steam_refresh_token"];

pub trait Store: Send + Sync {
    fn read(&self) -> Result<Option<String>, String>;
    fn write(&self, value: &str) -> Result<(), String>;
    fn clear(&self) -> Result<(), String>;
}

pub struct OsKeyring;

impl Store for OsKeyring {
    fn read(&self) -> Result<Option<String>, String> {
        let entry = keyring::Entry::new(SERVICE, ACCOUNT).map_err(|e| e.to_string())?;
        match entry.get_password() {
            Ok(v) => Ok(Some(v)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }

    fn write(&self, value: &str) -> Result<(), String> {
        keyring::Entry::new(SERVICE, ACCOUNT)
            .map_err(|e| e.to_string())?
            .set_password(value)
            .map_err(|e| e.to_string())
    }

    fn clear(&self) -> Result<(), String> {
        match keyring::Entry::new(SERVICE, ACCOUNT)
            .map_err(|e| e.to_string())?
            .delete_credential()
        {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }
}

pub struct Vault<S: Store = OsKeyring> {
    store: S,
    lock: Mutex<()>,
}

impl<S: Store> Vault<S> {
    pub const fn new(store: S) -> Self {
        Self {
            store,
            lock: Mutex::new(()),
        }
    }

    fn validate_key(key: &str) -> Result<(), String> {
        if ALLOWED_KEYS.contains(&key) {
            Ok(())
        } else {
            Err(format!("Unsupported secret key: {key}"))
        }
    }

    fn load_map(&self) -> Result<BTreeMap<String, String>, String> {
        match self.store.read()? {
            None => Ok(BTreeMap::new()),
            Some(raw) if raw.trim().is_empty() => Ok(BTreeMap::new()),
            Some(raw) => serde_json::from_str(&raw).map_err(|e| e.to_string()),
        }
    }

    pub fn get(&self, key: &str) -> Result<Option<String>, String> {
        Self::validate_key(key)?;
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        Ok(self.load_map()?.remove(key))
    }

    pub fn put(&self, key: &str, value: Option<&str>) -> Result<(), String> {
        Self::validate_key(key)?;
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut map = self.load_map()?;
        match value.map(str::trim).filter(|s| !s.is_empty()) {
            Some(v) => {
                map.insert(key.to_string(), v.to_string());
            }
            None => {
                map.remove(key);
            }
        }
        if map.is_empty() {
            self.store.clear()
        } else {
            let json = serde_json::to_string(&map).map_err(|e| e.to_string())?;
            self.store.write(&json)
        }
    }
}

pub static VAULT: LazyLock<Vault<OsKeyring>> = LazyLock::new(|| Vault::new(OsKeyring));

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct MemoryStore {
        value: Mutex<Option<String>>,
    }

    impl Store for MemoryStore {
        fn read(&self) -> Result<Option<String>, String> {
            Ok(self.value.lock().unwrap().clone())
        }
        fn write(&self, value: &str) -> Result<(), String> {
            *self.value.lock().unwrap() = Some(value.into());
            Ok(())
        }
        fn clear(&self) -> Result<(), String> {
            *self.value.lock().unwrap() = None;
            Ok(())
        }
    }

    #[test]
    fn stores_all_secrets_in_one_keychain_item() {
        let vault = Vault::new(MemoryStore::default());
        assert_eq!(vault.get("steam_username").unwrap(), None);
        vault.put("steam_username", Some("iantruong")).unwrap();
        vault.put("steam_refresh_token", Some("eyJ...")).unwrap();
        assert_eq!(
            vault.get("steam_username").unwrap().as_deref(),
            Some("iantruong")
        );
        let raw = vault.store.value.lock().unwrap().clone().unwrap();
        let map: BTreeMap<String, String> = serde_json::from_str(&raw).unwrap();
        assert_eq!(map.len(), 2);
        vault.put("steam_username", None).unwrap();
        vault.put("steam_refresh_token", None).unwrap();
        assert_eq!(*vault.store.value.lock().unwrap(), None);
    }

    #[test]
    fn rejects_unknown_keys_and_blank_values() {
        let vault = Vault::new(MemoryStore::default());
        assert!(vault.get("password").is_err());
        assert!(vault.put("password", Some("x")).is_err());
        vault.put("steam_username", Some("   ")).unwrap();
        assert_eq!(*vault.store.value.lock().unwrap(), None);
    }

    #[test]
    fn keychain_identity_matches_spec() {
        assert_eq!(SERVICE, "com.iantruong.truongpaper");
        assert_eq!(ACCOUNT, "vault");
    }
}
