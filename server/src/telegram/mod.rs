//! Connecting a user's Telegram account.
//!
//! Telegram's login is a conversation: ask for a code, the user reads it off
//! their phone, send it back, maybe answer a 2FA password too. The client
//! that asked for the code is the only one that can redeem it, and in
//! grammers that client is backed by a `SenderPool` whose runner must stay
//! spawned for the connection to live. So a half-finished login keeps its
//! client, its pool task and its session alive in memory between HTTP
//! requests — that is what `PendingLogins` holds.
//!
//! On success the session is exported (see `session_store`), encrypted with
//! the user's wrapping key, and written to Postgres. Plaintext never
//! reaches disk.

pub mod connect;
pub mod files;
pub mod folders;
pub mod remote;
pub mod qr;
pub mod phone;
pub mod routes;
pub mod session_store;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use grammers_client::{Client, SignInError};
use grammers_mtsender::SenderPool;
use grammers_session::storages::MemorySession;
// `Session` is needed in scope for `home_dc_id`/`set_home_dc_id`: they are
// trait methods, not inherent ones on MemorySession.
use grammers_session::{Session, SessionData};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use uuid::Uuid;

/// How long a half-finished login survives.
///
/// Telegram's codes expire in minutes, and each pending entry holds an open
/// connection, so there is nothing to gain by keeping them longer.
const PENDING_TTL: Duration = Duration::from_secs(10 * 60);

/// A live connection plus everything needed to tear it down cleanly.
///
/// The runner task and the handle are kept, not dropped: the pool's `run()`
/// future finishes as soon as every handle is gone, which would close the
/// connection out from under a login that is only half done.
pub struct Connection {
    pub client: Client,
    pub session: Arc<MemorySession>,
    handle: grammers_mtsender::SenderPoolHandle,
    task: JoinHandle<()>,
}

impl Connection {
    /// Opens a connection against a fresh or restored session.
    pub fn open(session: Arc<MemorySession>, api_id: i32) -> Self {
        let pool = SenderPool::new(Arc::clone(&session), api_id);
        let client = Client::new(&pool);
        let SenderPool { runner, handle, .. } = pool;
        let task = tokio::spawn(runner.run());

        Self {
            client,
            session,
            handle,
            task,
        }
    }

    /// Moves this connection to another datacenter.
    ///
    /// Telegram shards accounts across datacenters, and a connection
    /// starts on a default one. Asking about a number that lives
    /// elsewhere answers `PHONE_MIGRATE_n` rather than doing the work, and
    /// the request has to be made again against DC `n`.
    ///
    /// grammers' own wrappers do this internally, which is why the
    /// high-level API never surfaces it — but the raw calls this app makes
    /// to read the code delivery type have to handle it themselves.
    pub fn migrate_to(&self, dc_id: i32) {
        let old = self.session.home_dc_id();
        if old == dc_id {
            return;
        }
        // Dropping the old connection first frees a socket that will never
        // be used again, and lets the new datacenter export an
        // authorization cleanly.
        self.handle.disconnect_from_dc(old);
        self.session.set_home_dc_id(dc_id);
        tracing::info!(from = old, to = dc_id, "migrated datacenter");
    }

    /// Closes the connection and waits for its task to finish.
    pub async fn close(self) {
        drop(self.handle);
        drop(self.client);
        // The runner exits once the handles are gone. Awaiting it means a
        // closed connection is actually closed before we return, rather
        // than lingering as a detached task holding a socket.
        let _ = self.task.await;
    }
}

/// Where a connection attempt has got to.
pub enum LoginStage {
    /// A code has been sent; waiting for the user to type it. Carries the
    /// hash Telegram issued and how it said the code was delivered.
    AwaitingCode {
        phone_code_hash: String,
        delivery: phone::Delivery,
    },
    /// The code was right but the account has 2FA; waiting for the password.
    AwaitingPassword(Box<grammers_client::types::PasswordToken>),
    /// A QR code is on screen; waiting for someone to scan it.
    AwaitingQrScan,
}

pub struct Pending {
    pub connection: Connection,
    pub stage: LoginStage,
    pub api_id: i32,
    pub api_hash: String,
    pub phone: String,
    pub started: Instant,
}

/// In-flight logins, one per user.
#[derive(Default)]
pub struct PendingLogins {
    inner: Mutex<HashMap<Uuid, Pending>>,
    /// Users with a poll currently running.
    ///
    /// A QR poll removes the entry while it talks to Telegram, which can
    /// take seconds when a scan triggers a datacenter handshake. The
    /// browser polls on a fixed timer and does not know that, so a second
    /// poll arrives mid-handshake, finds the map empty, and reports the
    /// sign-in expired — cancelling a login that was about to succeed.
    /// This lets the overlapping poll answer "still waiting" instead.
    ///
    /// A std mutex rather than tokio's: it is only ever held for a set
    /// insert or remove, never across an await, and a synchronous lock is
    /// what lets the guard below release itself in `Drop`.
    polling: std::sync::Mutex<std::collections::HashSet<Uuid>>,
}

impl PendingLogins {
    pub fn new() -> Self {
        Self::default()
    }

    /// Stores an attempt, closing any earlier one for the same user. A user
    /// who restarts the wizard wants the previous half-finished attempt
    /// gone, and its connection released.
    pub async fn insert(&self, user: Uuid, pending: Pending) {
        let displaced = self.inner.lock().await.insert(user, pending);
        if let Some(old) = displaced {
            old.connection.close().await;
        }
    }

    /// Removes and returns an attempt, or `None` if it expired.
    ///
    /// An expired entry is closed rather than handed back: Telegram has
    /// already forgotten the token, so returning it would fail confusingly
    /// two steps later instead of clearly here.
    pub async fn take(&self, user: Uuid) -> Option<Pending> {
        let pending = self.inner.lock().await.remove(&user)?;
        if pending.started.elapsed() > PENDING_TTL {
            pending.connection.close().await;
            return None;
        }
        Some(pending)
    }

    pub async fn remove(&self, user: Uuid) {
        let removed = self.inner.lock().await.remove(&user);
        if let Some(pending) = removed {
            pending.connection.close().await;
        }
    }

    /// Marks a poll as running, or reports that one already is.
    ///
    /// Returns `false` when another poll holds the slot, which the caller
    /// should treat as "still waiting" rather than an error.
    pub fn begin_poll(&self, user: Uuid) -> bool {
        self.polling
            .lock()
            .map(|mut set| set.insert(user))
            .unwrap_or(true)
    }

    pub fn end_poll(&self, user: Uuid) {
        if let Ok(mut set) = self.polling.lock() {
            set.remove(&user);
        }
    }

    /// Closes attempts past their TTL. Each one holds a socket, so this
    /// matters more than sweeping idle keys does.
    pub async fn sweep(&self) -> usize {
        let expired: Vec<Pending> = {
            let mut map = self.inner.lock().await;
            let stale: Vec<Uuid> = map
                .iter()
                .filter(|(_, p)| p.started.elapsed() > PENDING_TTL)
                .map(|(id, _)| *id)
                .collect();
            stale.iter().filter_map(|id| map.remove(id)).collect()
        };

        let count = expired.len();
        for pending in expired {
            pending.connection.close().await;
        }
        count
    }

    pub async fn len(&self) -> usize {
        self.inner.lock().await.len()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum TelegramError {
    #[error("Could not reach Telegram. Check your API ID and hash, then try again.")]
    Connect,
    #[error("Telegram rejected that phone number.")]
    BadPhone,
    #[error("That code is not right.")]
    BadCode,
    #[error("That code has expired. Request a new one.")]
    ExpiredCode,
    #[error("That two-step verification password is not right.")]
    BadPassword,
    #[error("This phone number has no Telegram account. Sign up in the Telegram app first.")]
    NoAccount,
    #[error("Telegram asked us to wait {0} seconds before trying again.")]
    FloodWait(u32),
    #[error("Could not save the Telegram session.")]
    Session,
    #[error("No Telegram account is connected yet.")]
    NotConnected,
    /// The stored session exists but its key is not in memory — the server
    /// restarted, or the login went idle. Signing in again restores it.
    #[error("Please sign in again to unlock your Telegram connection.")]
    Locked,
    #[error("Telegram said: {0}")]
    Other(String),
}

/// Opens a connection and asks Telegram to send a login code.
pub async fn request_code(
    api_id: i32,
    api_hash: &str,
    phone: &str,
) -> Result<(Connection, grammers_client::types::LoginToken), TelegramError> {
    let session = Arc::new(MemorySession::from(SessionData::default()));
    let connection = Connection::open(session, api_id);

    match connection.client.request_login_code(phone, api_hash).await {
        Ok(token) => Ok((connection, token)),
        Err(error) => {
            // Close before returning, or a rejected attempt leaks a socket
            // and a task for every retry.
            connection.close().await;
            Err(classify(&error))
        }
    }
}

/// Maps grammers' error text onto something a user can act on.
///
/// grammers surfaces Telegram's raw RPC names; `PHONE_CODE_INVALID` is not
/// a useful thing to show someone who mistyped a digit.
fn classify(error: &grammers_client::InvocationError) -> TelegramError {
    let text = error.to_string();

    if let Some(seconds) = text
        .split("FLOOD_WAIT_")
        .nth(1)
        .and_then(|rest| rest.split(|c: char| !c.is_ascii_digit()).next())
        .and_then(|digits| digits.parse().ok())
    {
        return TelegramError::FloodWait(seconds);
    }

    match () {
        _ if text.contains("PHONE_NUMBER_INVALID") => TelegramError::BadPhone,
        _ if text.contains("PHONE_NUMBER_UNOCCUPIED") => TelegramError::NoAccount,
        _ if text.contains("PHONE_CODE_INVALID") => TelegramError::BadCode,
        _ if text.contains("PHONE_CODE_EXPIRED") => TelegramError::ExpiredCode,
        _ if text.contains("PASSWORD_HASH_INVALID") => TelegramError::BadPassword,
        _ if text.contains("API_ID_INVALID") || text.contains("API_ID_PUBLISHED") => {
            TelegramError::Connect
        }
        _ => TelegramError::Other(text),
    }
}

/// `classify` for callers outside this module.
pub fn classify_public(error: &grammers_client::InvocationError) -> TelegramError {
    classify(error)
}

fn classify_sign_in(error: SignInError) -> TelegramError {
    match error {
        SignInError::InvalidCode => TelegramError::BadCode,
        SignInError::InvalidPassword => TelegramError::BadPassword,
        SignInError::SignUpRequired { .. } => TelegramError::NoAccount,
        SignInError::Other(inner) => classify(&inner),
        other => TelegramError::Other(other.to_string()),
    }
}

/// Result of submitting a login code.
pub enum CodeOutcome {
    /// Signed in. Carries the exported session, ready to encrypt.
    SignedIn(Vec<u8>),
    /// The code was accepted but the account has 2FA.
    NeedsPassword(Box<grammers_client::types::PasswordToken>),
}

pub async fn submit_code(
    connection: &Connection,
    token: &grammers_client::types::LoginToken,
    code: &str,
) -> Result<CodeOutcome, TelegramError> {
    match connection.client.sign_in(token, code).await {
        Ok(_user) => Ok(CodeOutcome::SignedIn(export(connection)?)),
        Err(SignInError::PasswordRequired(password_token)) => {
            Ok(CodeOutcome::NeedsPassword(Box::new(password_token)))
        }
        Err(other) => Err(classify_sign_in(other)),
    }
}

pub async fn submit_password(
    connection: &Connection,
    token: grammers_client::types::PasswordToken,
    password: &str,
) -> Result<Vec<u8>, TelegramError> {
    connection
        .client
        .check_password(token, password)
        .await
        .map_err(classify_sign_in)?;

    export(connection)
}

fn export(connection: &Connection) -> Result<Vec<u8>, TelegramError> {
    session_store::export(&connection.session).map_err(|error| {
        tracing::error!(?error, "could not export the session after sign-in");
        TelegramError::Session
    })
}

pub type SharedPending = Arc<PendingLogins>;

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn an_unknown_pending_login_is_none() {
        let pending = PendingLogins::new();
        assert_eq!(pending.len().await, 0);
        assert!(pending.take(Uuid::new_v4()).await.is_none());
    }

    #[test]
    fn a_flood_wait_keeps_its_number() {
        // The figure matters: the UI tells the user how long to wait, and
        // "try later" is useless when it can be hours.
        let error = TelegramError::FloodWait(300);
        assert!(error.to_string().contains("300"));
    }

    #[test]
    fn every_user_facing_error_reads_as_an_instruction() {
        // These go straight into the UI, so none of them may leak a raw
        // Telegram RPC name.
        for error in [
            TelegramError::BadPhone,
            TelegramError::BadCode,
            TelegramError::ExpiredCode,
            TelegramError::BadPassword,
            TelegramError::NoAccount,
            TelegramError::Connect,
        ] {
            let text = error.to_string();
            assert!(!text.contains('_'), "{text:?} looks like an RPC name");
            assert!(text.ends_with('.'), "{text:?} is not a sentence");
        }
    }
}
