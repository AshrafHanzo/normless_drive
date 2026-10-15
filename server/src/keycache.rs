//! In-memory store of per-user wrapping keys, held only while a user is
//! signed in.
//!
//! A Telegram session is encrypted with a key derived from the user's
//! password (see `crate::crypto`). The password is visible to the server at
//! exactly one moment — the login request — and never again, because every
//! later request authenticates with a cookie. So the key is derived there
//! and kept here, keyed by the same session the cookie identifies.
//!
//! Consequences, both deliberate:
//!
//! * Logging out, or a restart, drops every key. The ciphertext in the
//!   database is then unreadable until the owner signs in again. That is the
//!   property being bought: a stolen database is inert.
//! * Nothing can act for a signed-out user. No 3am scheduled backup, no
//!   background sync. See docs/ARCHITECTURE.md section 2.1.
//!
//! Keys never touch disk and are wiped on drop by `WrappingKey`'s
//! `ZeroizeOnDrop`.

use std::collections::HashMap;
use std::sync::RwLock;
use std::time::{Duration, Instant};

use crate::crypto::WrappingKey;

/// How long an entry survives without being used.
///
/// Shorter than the cookie lifetime on purpose: an abandoned browser tab
/// should stop being able to decrypt long before its cookie expires.
const IDLE_TTL: Duration = Duration::from_secs(60 * 60 * 12);

struct Entry {
    key: WrappingKey,
    last_used: Instant,
}

#[derive(Default)]
pub struct KeyCache {
    // Keyed by the SHA-256 of the session token, the same value stored in
    // app_sessions — so the raw cookie is not held in memory either.
    entries: RwLock<HashMap<Vec<u8>, Entry>>,
}

impl KeyCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&self, session_hash: Vec<u8>, key: WrappingKey) {
        if let Ok(mut entries) = self.entries.write() {
            entries.insert(
                session_hash,
                Entry {
                    key,
                    last_used: Instant::now(),
                },
            );
        }
    }

    /// Returns the key for a session, refreshing its idle timer.
    ///
    /// `None` means the user must sign in again before anything encrypted
    /// can be read — which is a normal state after a restart, not an error.
    pub fn get(&self, session_hash: &[u8]) -> Option<WrappingKey> {
        let mut entries = self.entries.write().ok()?;
        let entry = entries.get_mut(session_hash)?;

        if entry.last_used.elapsed() > IDLE_TTL {
            entries.remove(session_hash);
            return None;
        }

        entry.last_used = Instant::now();
        Some(entry.key.clone())
    }

    pub fn remove(&self, session_hash: &[u8]) {
        if let Ok(mut entries) = self.entries.write() {
            entries.remove(session_hash);
        }
    }

    /// Drops every key for a user, across all their sessions.
    ///
    /// Used when a password changes or an account is suspended. Takes the
    /// session hashes rather than a user id because the cache is not indexed
    /// by user — the caller already has to query `app_sessions` to revoke
    /// them anyway.
    pub fn remove_many(&self, session_hashes: &[Vec<u8>]) {
        if let Ok(mut entries) = self.entries.write() {
            for hash in session_hashes {
                entries.remove(hash);
            }
        }
    }

    /// Drops entries that have gone idle. Called periodically so a key does
    /// not linger in memory for the full cookie lifetime after someone
    /// closes their laptop lid.
    pub fn sweep(&self) -> usize {
        let Ok(mut entries) = self.entries.write() else {
            return 0;
        };
        let before = entries.len();
        entries.retain(|_, entry| entry.last_used.elapsed() <= IDLE_TTL);
        before - entries.len()
    }

    pub fn len(&self) -> usize {
        self.entries.read().map(|e| e.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::{derive_key, generate_salt};

    fn a_key() -> WrappingKey {
        derive_key("correct horse battery staple", &generate_salt()).unwrap()
    }

    #[test]
    fn a_stored_key_comes_back() {
        let cache = KeyCache::new();
        cache.insert(b"session-a".to_vec(), a_key());
        assert!(cache.get(b"session-a").is_some());
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn an_unknown_session_has_no_key() {
        let cache = KeyCache::new();
        assert!(cache.get(b"never-seen").is_none());
    }

    #[test]
    fn removing_a_session_drops_its_key() {
        // This is what logout does: the ciphertext must stop being readable
        // the moment the user signs out.
        let cache = KeyCache::new();
        cache.insert(b"session-a".to_vec(), a_key());
        cache.remove(b"session-a");
        assert!(cache.get(b"session-a").is_none());
        assert!(cache.is_empty());
    }

    #[test]
    fn sessions_do_not_share_keys() {
        let cache = KeyCache::new();
        cache.insert(b"session-a".to_vec(), a_key());
        assert!(cache.get(b"session-b").is_none());
    }

    #[test]
    fn remove_many_clears_every_named_session() {
        // A password change revokes all of a user's sessions at once; each
        // one's key has to go with it.
        let cache = KeyCache::new();
        for name in [b"s1".to_vec(), b"s2".to_vec(), b"s3".to_vec()] {
            cache.insert(name, a_key());
        }
        cache.remove_many(&[b"s1".to_vec(), b"s3".to_vec()]);
        assert!(cache.get(b"s1").is_none());
        assert!(cache.get(b"s2").is_some());
        assert!(cache.get(b"s3").is_none());
    }

    #[test]
    fn a_fresh_entry_survives_a_sweep() {
        let cache = KeyCache::new();
        cache.insert(b"session-a".to_vec(), a_key());
        assert_eq!(cache.sweep(), 0);
        assert!(cache.get(b"session-a").is_some());
    }

    #[test]
    fn the_key_that_comes_back_still_decrypts() {
        // Clone-on-read must not produce a different key: round-tripping
        // real ciphertext is the only check that actually proves it.
        let salt = generate_salt();
        let key = derive_key("correct horse battery staple", &salt).unwrap();
        let (ciphertext, nonce) = crate::crypto::encrypt(&key, b"session data").unwrap();

        let cache = KeyCache::new();
        cache.insert(b"session-a".to_vec(), key);

        let fetched = cache.get(b"session-a").expect("should be cached");
        assert_eq!(
            crate::crypto::decrypt(&fetched, &ciphertext, &nonce).unwrap(),
            b"session data"
        );
    }
}
