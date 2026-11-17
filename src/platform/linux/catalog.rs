use std::collections::HashMap;
use std::path::Path;
use std::sync::OnceLock;
use std::time::Duration;

use secret_service::{EncryptionType, SecretService};
use sha2::{Digest, Sha256};
use vpn_infrastructure::client_store::CatalogProtection;
use vpn_presentation::session::ClientError;
use zeroize::Zeroizing;

use crate::catalog_cipher::{KEY_BYTES, SealedCatalogCipher};

type CatalogKey = Zeroizing<[u8; KEY_BYTES]>;
const SERVICE_TIMEOUT: Duration = Duration::from_secs(4);

pub struct LinuxCatalogProtection {
    identity: String,
    key: OnceLock<CatalogKey>,
}

impl LinuxCatalogProtection {
    pub fn new(directory: &Path) -> Self {
        let identity = format!(
            "{:x}",
            Sha256::digest(directory.as_os_str().as_encoded_bytes())
        );
        Self {
            identity,
            key: OnceLock::new(),
        }
    }

    fn cipher(&self, create: bool) -> Result<SealedCatalogCipher, ClientError> {
        if let Some(key) = self.key.get() {
            return Ok(SealedCatalogCipher::new(key));
        }
        // CatalogProtection is called on the client worker, outside a Tokio runtime.
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| ClientError::StorageUnavailable)?;
        let key = runtime
            .block_on(async { tokio::time::timeout(SERVICE_TIMEOUT, self.load_key(create)).await })
            .map_err(|_| ClientError::StorageUnavailable)??;
        let key = self.key.get_or_init(|| key);
        Ok(SealedCatalogCipher::new(key))
    }

    async fn load_key(&self, create: bool) -> Result<CatalogKey, ClientError> {
        let unavailable = ClientError::StorageUnavailable;
        let service = SecretService::connect(EncryptionType::Dh)
            .await
            .map_err(|_| unavailable)?;
        let attributes = HashMap::from([
            ("application", "com.kobaproduction.blazer"),
            ("catalog", self.identity.as_str()),
        ]);
        let matches = service
            .search_items(attributes.clone())
            .await
            .map_err(|_| unavailable)?;
        if !matches.locked.is_empty() {
            return Err(unavailable);
        }
        match matches.unlocked.as_slice() {
            [item] => {
                let secret = Zeroizing::new(item.get_secret().await.map_err(|_| unavailable)?);
                let key = secret
                    .as_slice()
                    .try_into()
                    .map_err(|_| ClientError::StorageCorrupt)?;
                Ok(Zeroizing::new(key))
            }
            [] if create => create_key(&service, attributes).await,
            _ => Err(unavailable),
        }
    }
}

impl CatalogProtection for LinuxCatalogProtection {
    fn protect(&self, plaintext: &[u8]) -> Result<Vec<u8>, ClientError> {
        self.cipher(true)?.seal(plaintext)
    }

    fn unprotect(&self, ciphertext: &[u8]) -> Result<Vec<u8>, ClientError> {
        // Never provision a replacement key when reading an existing catalog.
        self.cipher(false)?.open(ciphertext)
    }
}

async fn create_key(
    service: &SecretService<'_>,
    attributes: HashMap<&str, &str>,
) -> Result<CatalogKey, ClientError> {
    let unavailable = ClientError::StorageUnavailable;
    let collection = service
        .get_default_collection()
        .await
        .map_err(|_| unavailable)?;
    collection
        .ensure_unlocked()
        .await
        .map_err(|_| unavailable)?;
    let mut key = Zeroizing::new([0; KEY_BYTES]);
    getrandom::fill(key.as_mut()).map_err(|_| unavailable)?;
    collection
        .create_item(
            "Blazer catalog key",
            attributes,
            key.as_ref(),
            false,
            "application/octet-stream",
        )
        .await
        .map_err(|_| unavailable)?;
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires an isolated D-Bus session with an unlocked persistent Secret Service"]
    fn sealed_catalog_key_survives_new_protection_instances() {
        let root = std::env::var_os("BLAZER_TEST_KEYRING_DATA")
            .map(std::path::PathBuf::from)
            .expect("set BLAZER_TEST_KEYRING_DATA to a fresh isolated path");
        let first = LinuxCatalogProtection::new(&root);
        assert_eq!(
            first.unprotect(b"unreadable"),
            Err(ClientError::StorageUnavailable)
        );
        let sample = b"isolated catalog test";
        let sealed = first.protect(sample).unwrap();
        drop(first);
        let reopened = LinuxCatalogProtection::new(&root);
        assert_eq!(reopened.unprotect(&sealed).unwrap(), sample);
    }
}
