//! Secrets at rest.
//!
//! - `Sealer`: XChaCha20-Poly1305 with keys derived from `TURMITE_SECRET`. Seals TOTP secrets,
//!   webhook and IPN secrets, node URLs and Monero view keys.
//! - Hot keys: age. The API process encrypts to the public recipient. Only the worker holds the
//!   identity (encrypted with a passphrase), so only it can decrypt hot keys and the deposit seed.

use std::io::{Read, Write};

use chacha20poly1305::{
    XChaCha20Poly1305, XNonce,
    aead::{Aead, KeyInit},
};
use hkdf::Hkdf;
use rand::RngExt;
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::error::{Error, Result};

/// Seals small secrets with a key derived from the server secret.
#[derive(Clone)]
pub struct Sealer {
    key: [u8; 32],
}

impl Sealer {
    pub fn new(secret: &[u8; 32]) -> Self {
        let hk = Hkdf::<Sha256>::new(Some(b"turmite-seal-v1"), secret);
        let mut key = [0u8; 32];
        hk.expand(b"seal", &mut key).expect("32 bytes is a valid HKDF length");
        Self { key }
    }

    /// `nonce (24) || ciphertext`. `context` binds the value to its column.
    pub fn seal(&self, context: &str, plaintext: &[u8]) -> Vec<u8> {
        let cipher = XChaCha20Poly1305::new((&self.key).into());
        let mut nonce = [0u8; 24];
        rand::rng().fill(&mut nonce);
        let payload = chacha20poly1305::aead::Payload { msg: plaintext, aad: context.as_bytes() };
        let ct = cipher.encrypt(&XNonce::from(nonce), payload).expect("encryption cannot fail");
        let mut out = nonce.to_vec();
        out.extend_from_slice(&ct);
        out
    }

    pub fn open(&self, context: &str, sealed: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
        if sealed.len() < 24 + 16 {
            return Err(Error::Internal("sealed value too short".into()));
        }
        let cipher = XChaCha20Poly1305::new((&self.key).into());
        let (nonce, ct) = sealed.split_at(24);
        let payload = chacha20poly1305::aead::Payload { msg: ct, aad: context.as_bytes() };
        let nonce = XNonce::try_from(nonce).map_err(|_| Error::Internal("bad nonce".into()))?;
        cipher
            .decrypt(&nonce, payload)
            .map(Zeroizing::new)
            .map_err(|_| Error::Internal(format!("cannot open sealed {context}: wrong TURMITE_SECRET?")))
    }

    pub fn seal_str(&self, context: &str, plaintext: &str) -> Vec<u8> {
        self.seal(context, plaintext.as_bytes())
    }

    pub fn open_str(&self, context: &str, sealed: &[u8]) -> Result<Zeroizing<String>> {
        let bytes = self.open(context, sealed)?;
        String::from_utf8(bytes.to_vec()).map(Zeroizing::new).map_err(|_| Error::Internal(format!("sealed {context} is not UTF-8")))
    }

    /// Keyed hash for values that must be looked up but not stored in clear.
    pub fn mac(&self, context: &str, data: &[u8]) -> String {
        let mut h = Sha256::new();
        h.update(self.key);
        h.update(context.as_bytes());
        h.update([0]);
        h.update(data);
        hex::encode(h.finalize())
    }
}

/// Encrypt to an age recipient (`age1...`).
pub fn hot_encrypt(recipient: &str, plaintext: &[u8]) -> Result<Vec<u8>> {
    let recipient: age::x25519::Recipient =
        recipient.trim().parse().map_err(|e| Error::BadRequest(format!("TURMITE_HOT_RECIPIENT is not an age recipient: {e}")))?;
    age::encrypt(&recipient, plaintext).map_err(|e| Error::Internal(format!("age encrypt: {e}")))
}

/// The worker's unlocked age identity.
pub struct HotIdentity {
    identity: age::x25519::Identity,
}

impl HotIdentity {
    /// Load an identity file encrypted with `passphrase`.
    pub fn load(path: &str, passphrase: &str) -> Result<Self> {
        let encrypted = std::fs::read(path).map_err(|e| Error::BadRequest(format!("read {path}: {e}")))?;
        Self::from_encrypted(&encrypted, passphrase)
    }

    pub fn from_encrypted(encrypted: &[u8], passphrase: &str) -> Result<Self> {
        let identity = age::scrypt::Identity::new(passphrase.to_owned().into());
        let decryptor = age::Decryptor::new(encrypted).map_err(|e| Error::BadRequest(format!("hot identity file: {e}")))?;
        let mut reader = decryptor
            .decrypt(std::iter::once(&identity as &dyn age::Identity))
            .map_err(|_| Error::BadRequest("wrong hot-wallet passphrase".into()))?;
        let mut text = Zeroizing::new(String::new());
        reader.read_to_string(&mut text).map_err(|e| Error::BadRequest(format!("hot identity file: {e}")))?;
        let identity = text
            .lines()
            .find(|l| l.starts_with("AGE-SECRET-KEY-"))
            .ok_or_else(|| Error::BadRequest("hot identity file has no age secret key".into()))?
            .parse::<age::x25519::Identity>()
            .map_err(|e| Error::BadRequest(format!("hot identity: {e}")))?;
        Ok(Self { identity })
    }

    /// A fresh identity for tests and `--demo`.
    pub fn generate() -> Self {
        Self { identity: age::x25519::Identity::generate() }
    }

    pub fn recipient(&self) -> String {
        self.identity.to_public().to_string()
    }

    pub fn decrypt(&self, ciphertext: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
        age::decrypt(&self.identity, ciphertext).map(Zeroizing::new).map_err(|e| Error::Internal(format!("age decrypt: {e}")))
    }
}

/// New identity for `turmite init`: (recipient, identity file encrypted with `passphrase`).
pub fn new_hot_identity(passphrase: &str) -> Result<(String, Vec<u8>)> {
    use age::secrecy::ExposeSecret;
    let identity = age::x25519::Identity::generate();
    let recipient = identity.to_public().to_string();
    let body =
        Zeroizing::new(format!("# turmite hot-wallet identity\n# public key: {recipient}\n{}\n", identity.to_string().expose_secret()));
    let encryptor = age::Encryptor::with_user_passphrase(passphrase.to_owned().into());
    let mut out = Vec::new();
    let mut writer = encryptor.wrap_output(&mut out).map_err(|e| Error::Internal(e.to_string()))?;
    writer.write_all(body.as_bytes()).map_err(|e| Error::Internal(e.to_string()))?;
    writer.finish().map_err(|e| Error::Internal(e.to_string()))?;
    Ok((recipient, out))
}

/// Random bytes as lowercase hex.
pub fn random_hex(len: usize) -> String {
    let mut bytes = vec![0u8; len];
    rand::rng().fill(&mut bytes[..]);
    hex::encode(bytes)
}

pub fn sha256_hex(input: &[u8]) -> String {
    hex::encode(Sha256::digest(input))
}

/// Constant-time string comparison.
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seal_round_trip_and_context_binding() {
        let s = Sealer::new(&[1u8; 32]);
        let sealed = s.seal_str("totp", "JBSWY3DPEHPK3PXP");
        assert_eq!(&*s.open_str("totp", &sealed).unwrap(), "JBSWY3DPEHPK3PXP");
        assert!(s.open("webhook", &sealed).is_err());
        assert!(Sealer::new(&[2u8; 32]).open("totp", &sealed).is_err());
    }

    #[test]
    fn hot_identity_round_trip() {
        let (recipient, file) = new_hot_identity("correct horse battery").unwrap();
        let ct = hot_encrypt(&recipient, b"seed").unwrap();
        let id = HotIdentity::from_encrypted(&file, "correct horse battery").unwrap();
        assert_eq!(id.recipient(), recipient);
        assert_eq!(&**id.decrypt(&ct).unwrap(), b"seed");
        assert!(HotIdentity::from_encrypted(&file, "wrong").is_err());
    }
}
