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
    /// The decoded vault, read from the keychain at most once per launch. macOS asks for the
    /// keychain password on every read by a build it hasn't been told to trust, so repeated
    /// reads (each menu bar refresh checks the Steam account) would show the prompt again
    /// and again. A failed read (for example "Deny") is cached too, so it is not retried.
    cache: Mutex<Option<Result<BTreeMap<String, String>, String>>>,
}

impl<S: Store> Vault<S> {
    pub const fn new(store: S) -> Self {
        Self {
            store,
            cache: Mutex::new(None),
        }
    }

    fn validate_key(key: &str) -> Result<(), String> {
        if ALLOWED_KEYS.contains(&key) {
            Ok(())
        } else {
            Err(format!("Unsupported secret key: {key}"))
        }
    }

    fn decode(raw: Option<String>) -> Result<BTreeMap<String, String>, String> {
        match raw {
            None => Ok(BTreeMap::new()),
            Some(raw) if raw.trim().is_empty() => Ok(BTreeMap::new()),
            Some(raw) => serde_json::from_str(&raw).map_err(|e| e.to_string()),
        }
    }

    fn load_map(
        &self,
        cache: &mut Option<Result<BTreeMap<String, String>, String>>,
    ) -> Result<BTreeMap<String, String>, String> {
        cache
            .get_or_insert_with(|| self.store.read().and_then(Self::decode))
            .clone()
    }

    pub fn get(&self, key: &str) -> Result<Option<String>, String> {
        Self::validate_key(key)?;
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        Ok(self.load_map(&mut cache)?.remove(key))
    }

    pub fn put(&self, key: &str, value: Option<&str>) -> Result<(), String> {
        Self::validate_key(key)?;
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        // A cached failed read must not block a fresh sign-in, so retry the read once here.
        if matches!(*cache, Some(Err(_))) {
            *cache = None;
        }
        let mut map = self.load_map(&mut cache)?;
        match value.map(str::trim).filter(|s| !s.is_empty()) {
            Some(v) => {
                map.insert(key.to_string(), v.to_string());
            }
            None => {
                map.remove(key);
            }
        }
        if map.is_empty() {
            self.store.clear()?;
        } else {
            let json = serde_json::to_string(&map).map_err(|e| e.to_string())?;
            self.store.write(&json)?;
        }
        *cache = Some(Ok(map));
        Ok(())
    }

    /// Deletes the whole vault item without reading it.
    pub fn clear_all(&self) -> Result<(), String> {
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        self.store.clear()?;
        *cache = Some(Ok(BTreeMap::new()));
        Ok(())
    }
}

pub static VAULT: LazyLock<Vault<OsKeyring>> = LazyLock::new(|| Vault::new(OsKeyring));

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct MemoryStore {
        value: Mutex<Option<String>>,
        reads: std::sync::atomic::AtomicUsize,
    }

    impl Store for MemoryStore {
        fn read(&self) -> Result<Option<String>, String> {
            self.reads
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
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
    fn reads_the_keychain_once_per_launch() {
        let vault = Vault::new(MemoryStore::default());
        *vault.store.value.lock().unwrap() = Some(r#"{"steam_username":"ian"}"#.into());
        for _ in 0..5 {
            assert_eq!(vault.get("steam_username").unwrap().as_deref(), Some("ian"));
        }
        vault.put("steam_refresh_token", Some("t")).unwrap();
        assert_eq!(
            vault.get("steam_refresh_token").unwrap().as_deref(),
            Some("t")
        );
        assert_eq!(
            vault.store.reads.load(std::sync::atomic::Ordering::Relaxed),
            1
        );
    }

    #[test]
    fn keychain_identity_matches_spec() {
        assert_eq!(SERVICE, "com.iantruong.truongpaper");
        assert_eq!(ACCOUNT, "vault");
    }
}
