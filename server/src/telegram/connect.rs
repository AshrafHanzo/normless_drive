//! Getting a working Telegram client for a signed-in user.
//!
//! Every file operation needs one, and opening a fresh MTProto connection
//! per HTTP request would be far too slow — the handshake alone costs a
//! round trip to Telegram. So connections are cached per user and reused,
//! and closed once idle.
//!
//! Decryption needs the user's wrapping key, which only exists while they
//! are signed in (`crate::keycache`). A missing key is therefore not a
//! fault: it means "sign in again", and callers surface exactly that.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;
use uuid::Uuid;

use super::{session_store, Connection, TelegramError};
use crate::{crypto::WrappingKey, db::Db};

/// How long an unused connection is kept open.
///
/// Long enough that a burst of file operations reuses one connection,
/// short enough that an idle user is not holding a socket all day.
const IDLE_TTL: Duration = Duration::from_secs(10 * 60);

struct Live {
    connection: Arc<Connection>,
    last_used: Instant,
}

#[derive(Default)]
pub struct Clients {
    inner: Mutex<HashMap<Uuid, Live>>,
}

impl Clients {
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns a connected client for this user, opening one if needed.
    pub async fn get(
        &self,
        user: Uuid,
        key: &WrappingKey,
        pool: &Db,
    ) -> Result<Arc<Connection>, TelegramError> {
        {
            let mut map = self.inner.lock().await;
            if let Some(live) = map.get_mut(&user) {
                if live.last_used.elapsed() <= IDLE_TTL {
                    live.last_used = Instant::now();
                    return Ok(Arc::clone(&live.connection));
                }
            }
        }

        let connection = Arc::new(self.open(user, key, pool).await?);

        let displaced = self.inner.lock().await.insert(
            user,
            Live {
                connection: Arc::clone(&connection),
                last_used: Instant::now(),
            },
        );
        // Another request may have opened one concurrently. Whichever lost
        // the race is dropped here rather than leaked; `Arc` keeps it alive
        // for anyone already using it.
        drop(displaced);

        Ok(connection)
    }

    async fn open(
        &self,
        user: Uuid,
        key: &WrappingKey,
        pool: &Db,
    ) -> Result<Connection, TelegramError> {
        let client = pool
            .get()
            .await
            .map_err(|error| TelegramError::Other(format!("database unavailable: {error}")))?;

        let row = client
            .query_opt(
                "SELECT api_id, session_enc, session_nonce FROM tg_sessions WHERE user_id = $1",
                &[&user],
            )
            .await
            .map_err(|error| TelegramError::Other(format!("database error: {error}")))?
            .ok_or(TelegramError::NotConnected)?;

        let api_id: i32 = row.get("api_id");
        let session_enc: Vec<u8> = row.get("session_enc");
        let session_nonce: Vec<u8> = row.get("session_nonce");

        let plaintext = crate::crypto::decrypt(key, &session_enc, &session_nonce)
            .map_err(|_| TelegramError::Locked)?;

        let session = session_store::import(&plaintext).map_err(|error| {
            tracing::warn!(?error, %user, "stored session did not import");
            TelegramError::Session
        })?;

        Ok(Connection::open(session, api_id))
    }

    /// Drops a user's cached connection, e.g. on disconnect or logout.
    pub async fn evict(&self, user: Uuid) {
        let removed = self.inner.lock().await.remove(&user);
        if let Some(live) = removed {
            // Only close if nothing else is mid-request on it. If another
            // task still holds a clone, closing now would cut it off; the
            // sweep will collect it once that finishes.
            if let Ok(connection) = Arc::try_unwrap(live.connection) {
                connection.close().await;
            }
        }
    }

    /// Closes connections idle past the TTL.
    pub async fn sweep(&self) -> usize {
        let expired: Vec<Arc<Connection>> = {
            let mut map = self.inner.lock().await;
            let stale: Vec<Uuid> = map
                .iter()
                .filter(|(_, live)| live.last_used.elapsed() > IDLE_TTL)
                .map(|(id, _)| *id)
                .collect();
            stale
                .iter()
                .filter_map(|id| map.remove(id))
                .map(|live| live.connection)
                .collect()
        };

        let mut closed = 0;
        for connection in expired {
            if let Ok(owned) = Arc::try_unwrap(connection) {
                owned.close().await;
                closed += 1;
            }
        }
        closed
    }

    pub async fn len(&self) -> usize {
        self.inner.lock().await.len()
    }
}

pub type SharedClients = Arc<Clients>;

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_new_cache_is_empty() {
        let clients = Clients::new();
        assert_eq!(clients.len().await, 0);
        // Evicting something absent must not panic -- disconnect calls this
        // whether or not a connection was ever opened.
        clients.evict(Uuid::new_v4()).await;
        assert_eq!(clients.sweep().await, 0);
    }
}
