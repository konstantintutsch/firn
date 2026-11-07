//! Encrypts Garmin API keys at rest and makes the random tokens of sessions.

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use anyhow::{Result, bail};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::RngCore;
use sha2::{Digest, Sha256};

const NONCE_BYTES: usize = 12;
/// Short secrets could be guessed offline from a copy of the database.
pub const MIN_SECRET_LENGTH: usize = 32;

/// Holds the key derived from APP_SECRET. Changing the secret makes saved keys unreadable.
#[derive(Clone)]
pub struct SecretBox {
    cipher: Aes256Gcm,
}

impl SecretBox {
    pub fn new(secret: &str) -> Result<Self> {
        if secret.len() < MIN_SECRET_LENGTH {
            bail!("APP_SECRET must have at least {MIN_SECRET_LENGTH} characters");
        }
        let key = Sha256::digest(secret.as_bytes());
        Ok(Self {
            cipher: Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&key)),
        })
    }

    /// Returns a random nonce followed by the ciphertext.
    pub fn seal(&self, plaintext: &str) -> Result<Vec<u8>> {
        let mut nonce = [0; NONCE_BYTES];
        rand::rng().fill_bytes(&mut nonce);
        let ciphertext = self
            .cipher
            .encrypt(Nonce::from_slice(&nonce), plaintext.as_bytes())
            .map_err(|_| anyhow::anyhow!("could not encrypt"))?;
        Ok([nonce.as_slice(), &ciphertext].concat())
    }

    pub fn open(&self, sealed: &[u8]) -> Result<String> {
        if sealed.len() < NONCE_BYTES {
            bail!("sealed value is too short");
        }
        let (nonce, ciphertext) = sealed.split_at(NONCE_BYTES);
        let plaintext = self
            .cipher
            .decrypt(Nonce::from_slice(nonce), ciphertext)
            .map_err(|_| anyhow::anyhow!("could not decrypt. Did APP_SECRET change?"))?;
        Ok(String::from_utf8(plaintext)?)
    }
}

/// 256 random bits, safe to put in a cookie or URL.
pub fn random_token() -> String {
    let mut bytes = [0; 32];
    rand::rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

pub fn sha256(text: &str) -> Vec<u8> {
    sha256_bytes(text.as_bytes())
}

pub fn sha256_bytes(bytes: &[u8]) -> Vec<u8> {
    Sha256::digest(bytes).to_vec()
}

/// The PKCE code challenge for a verifier, with the S256 method.
pub fn code_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "a secret that is long enough for tests";

    #[test]
    fn opens_what_it_sealed() {
        let secret_box = SecretBox::new(SECRET).unwrap();
        let sealed = secret_box.seal("garmin-key").unwrap();
        assert!(!sealed.windows(10).any(|window| window == b"garmin-key"));
        assert_eq!(secret_box.open(&sealed).unwrap(), "garmin-key");
    }

    #[test]
    fn another_secret_cannot_open_it() {
        let sealed = SecretBox::new(SECRET).unwrap().seal("garmin-key").unwrap();
        let other = SecretBox::new("another secret that is long enough").unwrap();
        assert!(other.open(&sealed).is_err());
    }

    #[test]
    fn refuses_short_secrets() {
        assert!(SecretBox::new("short").is_err());
    }

    #[test]
    fn challenge_matches_rfc_7636_example() {
        assert_eq!(
            code_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }
}
