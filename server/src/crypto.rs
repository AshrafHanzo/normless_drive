//! Wrapping Telegram sessions with a key derived from the user's password.
//!
//! See docs/ARCHITECTURE.md section 2.1 for why this exists. The short
//! version: this server has to hold a credential equivalent to full access
//! to each connected user's Telegram account, so the least it can do is
//! ensure that a stolen copy of the database is not enough to use them.
//!
//! The key never touches disk. It is derived at login, held in memory for as
//! long as that login lasts, and dropped on logout. A database dump on its
//! own decrypts to nothing.
//!
//! What this does NOT defend against: code execution on a running server.
//! An attacker at that level reads keys out of memory for every user who is
//! currently signed in. This is data-at-rest protection, nothing more, and
//! claiming otherwise would be worse than not having it.

use argon2::Argon2;
use chacha20poly1305::{
    aead::{Aead, KeyInit, OsRng},
    XChaCha20Poly1305, XNonce,
};
use rand::RngCore;
use zeroize::{Zeroize, ZeroizeOnDrop};

/// Argon2id output, wiped when it goes out of scope.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct WrappingKey([u8; 32]);

impl std::fmt::Debug for WrappingKey {
    /// Never let a key reach a log line through `{:?}` on a containing struct.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("WrappingKey(<redacted>)")
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CryptoError {
    #[error("could not derive a key")]
    Kdf,
    #[error("could not encrypt")]
    Encrypt,
    /// Returned both for a wrong key and for tampered ciphertext. The two are
    /// deliberately indistinguishable to the caller.
    #[error("could not decrypt: wrong password or corrupted data")]
    Decrypt,
    #[error("salt must be {expected} bytes, got {actual}")]
    SaltLength { expected: usize, actual: usize },
}

pub const SALT_LEN: usize = 16;
pub const NONCE_LEN: usize = 24;

/// A fresh random salt for a new Telegram connection.
pub fn generate_salt() -> [u8; SALT_LEN] {
    let mut salt = [0u8; SALT_LEN];
    OsRng.fill_bytes(&mut salt);
    salt
}

/// Derives the wrapping key for a user from their plaintext password.
///
/// This salt must be distinct from the one inside the user's stored password
/// hash. Sharing it would tie the login verifier and the encryption key to
/// the same derivation, so cracking either would weaken both.
pub fn derive_key(password: &str, salt: &[u8]) -> Result<WrappingKey, CryptoError> {
    if salt.len() != SALT_LEN {
        return Err(CryptoError::SaltLength {
            expected: SALT_LEN,
            actual: salt.len(),
        });
    }
    let mut key = [0u8; 32];
    Argon2::default()
        .hash_password_into(password.as_bytes(), salt, &mut key)
        .map_err(|_| CryptoError::Kdf)?;
    Ok(WrappingKey(key))
}

/// Encrypts `plaintext`, returning `(ciphertext, nonce)`.
///
/// A fresh nonce every call. XChaCha20's 24-byte nonce is wide enough that
/// random generation will not collide in any realistic number of writes,
/// which is exactly why it is used here rather than ChaCha20's 12-byte one.
pub fn encrypt(key: &WrappingKey, plaintext: &[u8]) -> Result<(Vec<u8>, Vec<u8>), CryptoError> {
    let cipher = XChaCha20Poly1305::new(key.0.as_ref().into());
    let mut nonce_bytes = [0u8; NONCE_LEN];
    OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = XNonce::from_slice(&nonce_bytes);

    let ciphertext = cipher
        .encrypt(nonce, plaintext)
        .map_err(|_| CryptoError::Encrypt)?;

    Ok((ciphertext, nonce_bytes.to_vec()))
}

pub fn decrypt(key: &WrappingKey, ciphertext: &[u8], nonce: &[u8]) -> Result<Vec<u8>, CryptoError> {
    if nonce.len() != NONCE_LEN {
        return Err(CryptoError::Decrypt);
    }
    let cipher = XChaCha20Poly1305::new(key.0.as_ref().into());
    cipher
        .decrypt(XNonce::from_slice(nonce), ciphertext)
        .map_err(|_| CryptoError::Decrypt)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PASSWORD: &str = "correct horse battery staple";

    #[test]
    fn a_session_round_trips() {
        let salt = generate_salt();
        let key = derive_key(PASSWORD, &salt).unwrap();
        let session = b"1BQANOTEuMTA4LjU2LjEyOAG7..." as &[u8];

        let (ciphertext, nonce) = encrypt(&key, session).unwrap();
        assert_eq!(decrypt(&key, &ciphertext, &nonce).unwrap(), session);
    }

    #[test]
    fn the_wrong_password_cannot_decrypt() {
        let salt = generate_salt();
        let key = derive_key(PASSWORD, &salt).unwrap();
        let (ciphertext, nonce) = encrypt(&key, b"session").unwrap();

        let attacker = derive_key("hunter2", &salt).unwrap();
        assert!(decrypt(&attacker, &ciphertext, &nonce).is_err());
    }

    #[test]
    fn the_same_password_with_a_different_salt_is_a_different_key() {
        // Otherwise two users who picked the same password would share a
        // wrapping key, and either could decrypt the other's session.
        let (salt_a, salt_b) = (generate_salt(), generate_salt());
        let key_a = derive_key(PASSWORD, &salt_a).unwrap();
        let key_b = derive_key(PASSWORD, &salt_b).unwrap();

        let (ciphertext, nonce) = encrypt(&key_a, b"session").unwrap();
        assert!(decrypt(&key_b, &ciphertext, &nonce).is_err());
    }

    #[test]
    fn tampering_with_the_ciphertext_is_detected() {
        // Poly1305 is what makes this an authenticated cipher: a flipped bit
        // must fail loudly, not decrypt to different plaintext.
        let salt = generate_salt();
        let key = derive_key(PASSWORD, &salt).unwrap();
        let (mut ciphertext, nonce) = encrypt(&key, b"session").unwrap();

        ciphertext[0] ^= 0x01;
        assert!(decrypt(&key, &ciphertext, &nonce).is_err());
    }

    #[test]
    fn each_encryption_uses_a_fresh_nonce() {
        // Reusing a nonce under one key is catastrophic for a stream cipher,
        // so identical plaintext must still produce differing ciphertext.
        let salt = generate_salt();
        let key = derive_key(PASSWORD, &salt).unwrap();

        let (first, first_nonce) = encrypt(&key, b"same").unwrap();
        let (second, second_nonce) = encrypt(&key, b"same").unwrap();

        assert_ne!(first_nonce, second_nonce);
        assert_ne!(first, second);
    }

    #[test]
    fn a_truncated_nonce_is_refused_rather_than_padded() {
        let salt = generate_salt();
        let key = derive_key(PASSWORD, &salt).unwrap();
        let (ciphertext, _) = encrypt(&key, b"session").unwrap();
        assert!(decrypt(&key, &ciphertext, &[0u8; 8]).is_err());
    }

    #[test]
    fn a_wrong_length_salt_is_rejected() {
        assert!(matches!(
            derive_key(PASSWORD, &[0u8; 4]),
            Err(CryptoError::SaltLength { .. })
        ));
    }

    #[test]
    fn the_key_does_not_appear_in_debug_output() {
        let key = derive_key(PASSWORD, &generate_salt()).unwrap();
        assert_eq!(format!("{key:?}"), "WrappingKey(<redacted>)");
    }
}
