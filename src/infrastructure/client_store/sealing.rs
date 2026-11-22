use chacha20poly1305::aead::{Aead, Payload};
use chacha20poly1305::{KeyInit, XChaCha20Poly1305, XNonce};
use vpn_domain::client::ClientError;

const FORMAT: &[u8] = b"BZLC01\0";
const NONCE_BYTES: usize = 24;
const TAG_BYTES: usize = 16;
pub const KEY_BYTES: usize = 32;

pub struct SealedCatalogCipher(XChaCha20Poly1305);

impl SealedCatalogCipher {
    pub fn new(key: &[u8; KEY_BYTES]) -> Self {
        Self(XChaCha20Poly1305::new(key.into()))
    }

    pub fn seal(&self, plaintext: &[u8]) -> Result<Vec<u8>, ClientError> {
        let mut nonce = [0; NONCE_BYTES];
        getrandom::fill(&mut nonce).map_err(|_| ClientError::StorageUnavailable)?;
        let ciphertext = self
            .0
            .encrypt(
                &nonce.into(),
                Payload {
                    msg: plaintext,
                    aad: FORMAT,
                },
            )
            .map_err(|_| ClientError::StorageWrite)?;
        let mut sealed = Vec::with_capacity(FORMAT.len() + nonce.len() + ciphertext.len());
        sealed.extend_from_slice(FORMAT);
        sealed.extend_from_slice(&nonce);
        sealed.extend_from_slice(&ciphertext);
        Ok(sealed)
    }

    pub fn open(&self, sealed: &[u8]) -> Result<Vec<u8>, ClientError> {
        let body = sealed
            .strip_prefix(FORMAT)
            .filter(|body| body.len() >= NONCE_BYTES + TAG_BYTES)
            .ok_or(ClientError::StorageCorrupt)?;
        let (nonce, ciphertext) = body.split_at(NONCE_BYTES);
        self.0
            .decrypt(
                XNonce::from_slice(nonce),
                Payload {
                    msg: ciphertext,
                    aad: FORMAT,
                },
            )
            .map_err(|_| ClientError::StorageCorrupt)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_nonce_and_authentication_protect_the_catalog_envelope() {
        let cipher = SealedCatalogCipher::new(&[7; KEY_BYTES]);
        let plaintext = b"private test catalog";
        let mut sealed = cipher.seal(plaintext).unwrap();
        assert_ne!(sealed, cipher.seal(plaintext).unwrap());
        assert_eq!(cipher.open(&sealed).unwrap(), plaintext);
        assert_eq!(
            SealedCatalogCipher::new(&[9; KEY_BYTES]).open(&sealed),
            Err(ClientError::StorageCorrupt)
        );
        assert_eq!(
            cipher.open(&sealed[..FORMAT.len() + NONCE_BYTES]),
            Err(ClientError::StorageCorrupt)
        );
        let last = sealed.last_mut().unwrap();
        *last ^= 1;
        assert_eq!(cipher.open(&sealed), Err(ClientError::StorageCorrupt));
    }
}
