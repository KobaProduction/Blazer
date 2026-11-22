//! Probe preferences stored inside the protected client catalog.

use serde::{Deserialize, Serialize};
use vpn_domain::client::{ClientError, ProbeHttpMethod, ProbeMethod, ProbeSettings, ProbeStrategy};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ProbeSettingsDto {
    method: MethodDto,
    http_method: HttpMethodDto,
    url: String,
    timeout_ms: u32,
    attempts: u8,
    warmup: bool,
    strategy: StrategyDto,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum MethodDto {
    Automatic,
    Tcp,
    Icmp,
    Proxy,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum HttpMethodDto {
    Get,
    Head,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum StrategyDto {
    Last,
    Minimum,
    Average,
    Median,
}

impl ProbeSettingsDto {
    pub(super) fn from_settings(settings: &ProbeSettings) -> Option<Self> {
        if settings == &ProbeSettings::default() {
            return None;
        }
        Some(Self {
            method: match settings.method {
                ProbeMethod::Automatic => MethodDto::Automatic,
                ProbeMethod::Tcp => MethodDto::Tcp,
                ProbeMethod::Icmp => MethodDto::Icmp,
                ProbeMethod::Proxy => MethodDto::Proxy,
            },
            http_method: match settings.http_method {
                ProbeHttpMethod::Get => HttpMethodDto::Get,
                ProbeHttpMethod::Head => HttpMethodDto::Head,
            },
            url: settings.url.clone(),
            timeout_ms: settings.timeout_ms,
            attempts: settings.attempts,
            warmup: settings.warmup,
            strategy: match settings.strategy {
                ProbeStrategy::Last => StrategyDto::Last,
                ProbeStrategy::Minimum => StrategyDto::Minimum,
                ProbeStrategy::Average => StrategyDto::Average,
                ProbeStrategy::Median => StrategyDto::Median,
            },
        })
    }

    pub(super) fn into_settings(self) -> Result<ProbeSettings, ClientError> {
        let settings = ProbeSettings {
            method: match self.method {
                MethodDto::Automatic => ProbeMethod::Automatic,
                MethodDto::Tcp => ProbeMethod::Tcp,
                MethodDto::Icmp => ProbeMethod::Icmp,
                MethodDto::Proxy => ProbeMethod::Proxy,
            },
            http_method: match self.http_method {
                HttpMethodDto::Get => ProbeHttpMethod::Get,
                HttpMethodDto::Head => ProbeHttpMethod::Head,
            },
            url: self.url,
            timeout_ms: self.timeout_ms,
            attempts: self.attempts,
            warmup: self.warmup,
            strategy: match self.strategy {
                StrategyDto::Last => ProbeStrategy::Last,
                StrategyDto::Minimum => ProbeStrategy::Minimum,
                StrategyDto::Average => ProbeStrategy::Average,
                StrategyDto::Median => ProbeStrategy::Median,
            },
        };
        settings
            .validate()
            .map_err(|_| ClientError::StorageCorrupt)?;
        Ok(settings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client_store::{CatalogProtection, HEADER, SealedCatalogStore, StoredCatalog};
    use std::path::PathBuf;
    use vpn_application::client::StateStorePort;
    use vpn_domain::client::Catalog;

    struct FixtureDirectory(PathBuf);

    impl FixtureDirectory {
        fn new() -> Self {
            Self(std::env::temp_dir().join(format!("blazer-probe-store-{}", uuid::Uuid::new_v4())))
        }

        fn store(&self) -> SealedCatalogStore {
            SealedCatalogStore::with_protection(self.0.clone(), Box::new(FixtureProtection))
                .unwrap()
        }
    }

    impl Drop for FixtureDirectory {
        fn drop(&mut self) {
            for name in ["catalog.sealed", "catalog.lock"] {
                let _ = std::fs::remove_file(self.0.join(name));
            }
            let _ = std::fs::remove_dir(&self.0);
        }
    }

    struct FixtureProtection;

    impl CatalogProtection for FixtureProtection {
        fn protect(&self, plaintext: &[u8]) -> Result<Vec<u8>, ClientError> {
            Ok(plaintext.to_vec())
        }

        fn unprotect(&self, ciphertext: &[u8]) -> Result<Vec<u8>, ClientError> {
            Ok(ciphertext.to_vec())
        }
    }

    #[test]
    fn probe_preferences_round_trip_through_the_catalog_store() {
        let directory = FixtureDirectory::new();
        let store = directory.store();
        for (method, strategy) in [
            (ProbeMethod::Automatic, ProbeStrategy::Last),
            (ProbeMethod::Tcp, ProbeStrategy::Minimum),
            (ProbeMethod::Icmp, ProbeStrategy::Average),
            (ProbeMethod::Proxy, ProbeStrategy::Median),
        ] {
            let mut catalog = Catalog::default();
            catalog.settings.probe = ProbeSettings {
                method,
                http_method: ProbeHttpMethod::Head,
                url: "https://connectivity.example/check?format=204".into(),
                timeout_ms: 4_000,
                attempts: 3,
                warmup: false,
                strategy,
            };
            store.save(&catalog).unwrap();
            let restored = store.load().unwrap().unwrap().settings.probe;
            assert_eq!(restored, catalog.settings.probe);
            assert!(!restored.warmup);
            assert!(restored.http_warmup_required());
        }
    }

    #[test]
    fn absent_probe_preferences_use_the_current_default() {
        let catalog = Catalog::default();
        let encoded = serde_json::to_value(StoredCatalog::from_catalog(&catalog)).unwrap();
        assert!(encoded.get("probe").is_none());
        let restored: StoredCatalog = serde_json::from_value(encoded).unwrap();
        assert_eq!(catalog.settings.probe.http_method, ProbeHttpMethod::Head);
        assert_eq!(catalog.settings.probe.attempts, 2);
        assert_eq!(
            restored.into_catalog().unwrap().settings.probe,
            ProbeSettings::default()
        );
    }

    #[test]
    fn invalid_probe_preferences_cannot_replace_a_saved_catalog() {
        let directory = FixtureDirectory::new();
        let store = directory.store();
        let mut catalog = Catalog::default();
        store.save(&catalog).unwrap();
        let previous = std::fs::read(store.path()).unwrap();
        catalog.settings.probe.attempts = 0;
        assert_eq!(store.save(&catalog), Err(ClientError::InvalidInput));
        assert_eq!(std::fs::read(store.path()).unwrap(), previous);
    }

    #[test]
    fn invalid_stored_probe_preferences_are_reported_as_corrupt() {
        let directory = FixtureDirectory::new();
        let store = directory.store();
        let mut catalog = Catalog::default();
        catalog.settings.probe.timeout_ms = 1200;
        let baseline = serde_json::to_value(StoredCatalog::from_catalog(&catalog)).unwrap();
        assert!(baseline["probe"].is_object());
        for (field, value) in [
            ("timeout_ms", serde_json::json!(249)),
            ("timeout_ms", serde_json::json!(10_001)),
            ("attempts", serde_json::json!(0)),
            ("attempts", serde_json::json!(6)),
            ("url", serde_json::json!("file:///private")),
            ("method", serde_json::json!("unknown")),
            ("strategy", serde_json::json!("unknown")),
        ] {
            let mut encoded = baseline.clone();
            encoded["probe"][field] = value;
            let mut bytes = HEADER.to_vec();
            bytes.extend_from_slice(&serde_json::to_vec(&encoded).unwrap());
            std::fs::write(store.path(), bytes).unwrap();
            assert_eq!(store.load(), Err(ClientError::StorageCorrupt));
        }
    }
}
