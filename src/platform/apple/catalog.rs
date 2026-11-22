use std::sync::OnceLock;

use security_framework::access_control::{ProtectionMode, SecAccessControl};
use security_framework::passwords::{
    PasswordOptions, generic_password, set_generic_password_options,
};
use security_framework_sys::base::errSecItemNotFound;
use vpn_infrastructure::client_store::{
    CatalogProtection,
    sealing::{KEY_BYTES, SealedCatalogCipher},
};
use vpn_presentation::session::ClientError;
use zeroize::Zeroizing;

type CatalogKey = Zeroizing<[u8; KEY_BYTES]>;

#[derive(Default)]
pub struct AppleCatalogProtection {
    key: OnceLock<CatalogKey>,
}

impl AppleCatalogProtection {
    fn cipher(&self, create: bool) -> Result<SealedCatalogCipher, ClientError> {
        if let Some(key) = self.key.get() {
            return Ok(SealedCatalogCipher::new(key));
        }
        let key = match generic_password(key_options()) {
            Ok(secret) => decode_key(Zeroizing::new(secret))?,
            Err(error) if create && error.code() == errSecItemNotFound => create_key()?,
            Err(_) => return Err(ClientError::StorageUnavailable),
        };
        Ok(SealedCatalogCipher::new(self.key.get_or_init(|| key)))
    }
}

impl CatalogProtection for AppleCatalogProtection {
    fn protect(&self, plaintext: &[u8]) -> Result<Vec<u8>, ClientError> {
        self.cipher(true)?.seal(plaintext)
    }

    fn unprotect(&self, ciphertext: &[u8]) -> Result<Vec<u8>, ClientError> {
        self.cipher(false)?.open(ciphertext)
    }
}

fn key_options() -> PasswordOptions {
    let mut options = PasswordOptions::new_generic_password("com.kobaproduction.blazer", "catalog");
    options.use_protected_keychain();
    options
}

fn decode_key(secret: Zeroizing<Vec<u8>>) -> Result<CatalogKey, ClientError> {
    let key = secret
        .as_slice()
        .try_into()
        .map_err(|_| ClientError::StorageCorrupt)?;
    Ok(Zeroizing::new(key))
}

fn create_key() -> Result<CatalogKey, ClientError> {
    let unavailable = ClientError::StorageUnavailable;
    let mut key = Zeroizing::new([0; KEY_BYTES]);
    getrandom::fill(&mut *key).map_err(|_| unavailable)?;
    let access = SecAccessControl::create_with_protection(
        Some(ProtectionMode::AccessibleAfterFirstUnlockThisDeviceOnly),
        0,
    )
    .map_err(|_| unavailable)?;
    let mut options = key_options();
    options.set_access_control(access);
    set_generic_password_options(&*key, options).map_err(|_| unavailable)?;
    Ok(key)
}
