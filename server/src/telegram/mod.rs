//! Connecting a user's Telegram account.
//!
//! Telegram's login is a multi-step conversation: ask for a code, the user
//! reads it off their phone, send it back, and possibly answer a 2FA
//! password as well. The client that asked for the code is the only one that
//! can redeem it — the login token is bound to that connection — so the
//! half-finished client has to stay alive in memory between HTTP requests.
//! That is what `PendingLogins` is for.
//!
//! Once sign-in succeeds the session is encrypted with the user's wrapping
//! key (`crate::keycache`) and written to `tg_sessions`. The plaintext
//! session never touches disk.

pub mod routes;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use grammers_client::{Client, Config, SignInError};
use grammers_session::Session;
use tokio::sync::Mutex;
use uuid::Uuid;

/// How long a half-finished login survives.
///
/// Telegram's own codes expire in minutes, and each pending entry holds an
/// open connection, so there is nothing to gain by keeping them longer.
const PENDING_TTL: Duration = Duration::from_secs(10 * 60);

/// Where a connection attempt has got to.
pub enum LoginStage {
    /// A code has been sent; waiting for the user to type it.
    AwaitingCode(grammers_client::types::LoginToken),
    /// The code was right but the account has 2FA; waiting for the password.
    AwaitingPassword(grammers_client::types::PasswordToken),
}

pub struct Pending {
    pub client: Client,
    pub stage: LoginStage,
    pub api_id: i32,
    pub api_hash: String,
    pub phone: String,
    pub started: Instant,
}

/// In-flight logins, one per user.
///
/// A second attempt replaces the first rather than queueing: a user who
/// restarts the wizard wants the old half-finished attempt gone.
#[derive(Default)]
pub struct PendingLogins {
    inner: Mutex<HashMap<Uuid, Pending>>,
}

impl PendingLogins {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn insert(&self, user: Uuid, pending: Pending) {
        self.inner.lock().await.insert(user, pending);
    }

    pub async fn take(&self, user: Uuid) -> Option<Pending> {
        let mut map = self.inner.lock().await;
        let pending = map.remove(&user)?;
        if pending.started.elapsed() > PENDING_TTL {
            // Expired. Dropping it closes the connection, and the caller is
            // told to start again rather than given a token Telegram has
            // already forgotten.
            return None;
        }
        Some(pending)
    }

    pub async fn remove(&self, user: Uuid) {
        self.inner.lock().await.remove(&user);
    }

    /// Drops entries past their TTL, closing their connections.
    pub async fn sweep(&self) -> usize {
        let mut map = self.inner.lock().await;
        let before = map.len();
        map.retain(|_, pending| pending.started.elapsed() <= PENDING_TTL);
        before - map.len()
    }

    pub async fn len(&self) -> usize {
        self.inner.lock().await.len()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum TelegramError {
    #[error("Could not reach Telegram. Check the API ID and hash, then try again.")]
    Connect,
    #[error("Telegram rejected that phone number.")]
    BadPhone,
    #[error("That code is not right.")]
    BadCode,
    #[error("That code has expired. Request a new one.")]
    ExpiredCode,
    #[error("That two-step verification password is not right.")]
    BadPassword,
    #[error("This account has two-step verification enabled.")]
    PasswordRequired,
    #[error("Telegram is asking you to sign up rather than sign in — this phone number has no Telegram account.")]
    NoAccount,
    #[error("Telegram asked us to wait {0} seconds before trying again.")]
    FloodWait(u32),
    #[error("Telegram said: {0}")]
    Other(String),
}

/// Opens a connection and asks Telegram to send a login code.
pub async fn request_code(
    api_id: i32,
    api_hash: &str,
    phone: &str,
) -> Result<(Client, grammers_client::types::LoginToken), TelegramError> {
    let client = Client::connect(Config {
        session: Session::new(),
        api_id,
        api_hash: api_hash.to_string(),
        params: Default::default(),
    })
    .await
    .map_err(|error| {
        tracing::warn!(?error, "telegram connect failed");
        TelegramError::Connect
    })?;

    let token = client
        .request_login_code(phone)
        .await
        .map_err(|error| classify(&error))?;

    Ok((client, token))
}

/// Maps grammers' error text onto something a user can act on.
///
/// grammers surfaces Telegram's raw RPC error names; showing
/// `PHONE_CODE_INVALID` to someone who mistyped a digit is not helpful.
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
        _ if text.contains("PHONE_CODE_INVALID") => TelegramError::BadCode,
        _ if text.contains("PHONE_CODE_EXPIRED") => TelegramError::ExpiredCode,
        _ if text.contains("PASSWORD_HASH_INVALID") => TelegramError::BadPassword,
        _ if text.contains("SESSION_PASSWORD_NEEDED") => TelegramError::PasswordRequired,
        _ => TelegramError::Other(text),
    }
}

/// Result of submitting a login code.
pub enum CodeOutcome {
    /// Signed in. Carries the serialised session, ready to encrypt.
    SignedIn(Vec<u8>),
    /// The code was accepted but the account has 2FA.
    NeedsPassword(grammers_client::types::PasswordToken),
}

pub async fn submit_code(
    client: &Client,
    token: &grammers_client::types::LoginToken,
    code: &str,
) -> Result<CodeOutcome, TelegramError> {
    match client.sign_in(token, code).await {
        Ok(_user) => Ok(CodeOutcome::SignedIn(save_session(client)?)),
        Err(SignInError::PasswordRequired(password_token)) => {
            Ok(CodeOutcome::NeedsPassword(password_token))
        }
        Err(SignInError::InvalidCode) => Err(TelegramError::BadCode),
        Err(SignInError::SignUpRequired { .. }) => Err(TelegramError::NoAccount),
        Err(SignInError::InvalidPassword) => Err(TelegramError::BadPassword),
        Err(SignInError::Other(error)) => Err(classify(&error)),
        Err(other) => Err(TelegramError::Other(other.to_string())),
    }
}

pub async fn submit_password(
    client: &Client,
    token: grammers_client::types::PasswordToken,
    password: &str,
) -> Result<Vec<u8>, TelegramError> {
    client
        .check_password(token, password)
        .await
        .map_err(|error| match error {
            SignInError::InvalidPassword => TelegramError::BadPassword,
            SignInError::Other(inner) => classify(&inner),
            other => TelegramError::Other(other.to_string()),
        })?;

    save_session(client)
}

fn save_session(client: &Client) -> Result<Vec<u8>, TelegramError> {
    let bytes = client.session().save();
    if bytes.is_empty() {
        // Storing an empty session would look like success and fail later
        // with nothing to explain it.
        return Err(TelegramError::Other(
            "Telegram returned an empty session".into(),
        ));
    }
    Ok(bytes)
}

/// Rebuilds a client from a stored session, for ordinary use after connecting.
pub async fn client_from_session(
    session_bytes: &[u8],
    api_id: i32,
    api_hash: &str,
) -> Result<Client, TelegramError> {
    let session = Session::load(session_bytes).map_err(|error| {
        tracing::warn!(?error, "stored session did not load");
        TelegramError::Other("The stored Telegram session is unreadable.".into())
    })?;

    Client::connect(Config {
        session,
        api_id,
        api_hash: api_hash.to_string(),
        params: Default::default(),
    })
    .await
    .map_err(|error| {
        tracing::warn!(?error, "reconnect with stored session failed");
        TelegramError::Connect
    })
}

pub type SharedPending = Arc<PendingLogins>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flood_wait_seconds_are_pulled_out_of_the_message() {
        // The number matters: the UI tells the user how long to wait, and
        // "try again later" with no figure is useless when it can be hours.
        let classified = classify(&grammers_client::InvocationError::Rpc(
            grammers_client::types::RpcError {
                code: 420,
                name: "FLOOD_WAIT_300".to_string(),
                value: Some(300),
                caused_by: None,
            },
        ));
        assert!(matches!(classified, TelegramError::FloodWait(300)));
    }

    #[test]
    fn a_mistyped_code_reads_as_a_mistyped_code() {
        let classified = classify(&grammers_client::InvocationError::Rpc(
            grammers_client::types::RpcError {
                code: 400,
                name: "PHONE_CODE_INVALID".to_string(),
                value: None,
                caused_by: None,
            },
        ));
        assert!(matches!(classified, TelegramError::BadCode));
        assert_eq!(classified.to_string(), "That code is not right.");
    }

    #[tokio::test]
    async fn an_expired_pending_login_is_not_handed_back() {
        // Telegram has already forgotten the token by then; returning it
        // would produce a confusing failure two steps later instead of a
        // clear "start again" here.
        let pending = PendingLogins::new();
        assert_eq!(pending.len().await, 0);
        assert!(pending.take(Uuid::new_v4()).await.is_none());
    }
}
