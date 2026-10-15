//! Accounts, passwords and login sessions for the web app itself.
//!
//! Distinct from a Telegram session (see `crate::crypto`): this is only about
//! who is signed in to Normless Drive.

pub mod extract;
pub mod routes;

use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::Digest;
use uuid::Uuid;

/// Name of the cookie carrying the login session token.
pub const SESSION_COOKIE: &str = "nd_session";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Admin,
    User,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Admin => "admin",
            Role::User => "user",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "admin" => Some(Role::Admin),
            "user" => Some(Role::User),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Pending,
    Active,
    Suspended,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Pending => "pending",
            Status::Active => "active",
            Status::Suspended => "suspended",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "pending" => Some(Status::Pending),
            "active" => Some(Status::Active),
            "suspended" => Some(Status::Suspended),
            _ => None,
        }
    }

    /// Only an active account may hold a session. Pending means an admin has
    /// not approved the signup yet; suspended means approval was withdrawn.
    pub fn may_sign_in(self) -> bool {
        matches!(self, Status::Active)
    }
}

/// The signed-in user, attached to a request by the auth middleware.
#[derive(Debug, Clone)]
pub struct CurrentUser {
    pub id: Uuid,
    pub email: String,
    pub role: Role,
}

impl CurrentUser {
    pub fn is_admin(&self) -> bool {
        self.role == Role::Admin
    }
}

// ---------------------------------------------------------------- passwords

#[derive(Debug, thiserror::Error)]
pub enum PasswordError {
    #[error("could not hash the password")]
    Hash,
    #[error("stored password hash is unreadable")]
    MalformedHash,
}

pub fn hash_password(password: &str) -> Result<String, PasswordError> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|_| PasswordError::Hash)
}

/// Verifies a password against a stored PHC string.
///
/// A malformed stored hash is an error rather than `false`, so a corrupted
/// row shows up as a fault to investigate instead of looking to the user
/// like they simply mistyped.
pub fn verify_password(password: &str, stored: &str) -> Result<bool, PasswordError> {
    let parsed = PasswordHash::new(stored).map_err(|_| PasswordError::MalformedHash)?;
    Ok(Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok())
}

/// Minimum viable password rules. Deliberately length-first: length beats
/// character-class rules, which mostly produce `Password1!`.
pub fn validate_password(password: &str) -> Result<(), String> {
    if password.chars().count() < 12 {
        return Err("Password must be at least 12 characters.".into());
    }
    if password.chars().count() > 256 {
        return Err("Password must be at most 256 characters.".into());
    }
    Ok(())
}

pub fn normalize_email(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("Email is required.".into());
    }
    if trimmed.len() > 320 {
        return Err("That email address is too long.".into());
    }
    // Not full RFC 5322 — that is a famously bad idea to hand-roll. This
    // only rejects input that cannot possibly be deliverable; real
    // verification is "we sent you a link".
    let parts: Vec<&str> = trimmed.split('@').collect();
    if parts.len() != 2 || parts[0].is_empty() || parts[1].is_empty() || !parts[1].contains('.') {
        return Err("That does not look like an email address.".into());
    }
    Ok(trimmed.to_string())
}

// ----------------------------------------------------------- session tokens

/// A freshly minted session token: the raw value goes in the cookie, only
/// the hash is stored, so a dump of `app_sessions` cannot resume anyone's
/// login.
pub struct SessionToken {
    pub raw: String,
    pub hash: Vec<u8>,
}

pub fn generate_session_token() -> SessionToken {
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    let raw = hex::encode(bytes);
    let hash = hash_session_token(&raw);
    SessionToken { raw, hash }
}

/// SHA-256 rather than Argon2: this is a 256-bit random value, not a
/// human-chosen password, so there is nothing to brute force and a slow hash
/// would only add latency to every authenticated request.
pub fn hash_session_token(raw: &str) -> Vec<u8> {
    let mut hasher = sha2::Sha256::new();
    hasher.update(raw.as_bytes());
    hasher.finalize().to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_password_verifies_against_its_own_hash() {
        let hash = hash_password("correct horse battery staple").unwrap();
        assert!(verify_password("correct horse battery staple", &hash).unwrap());
        assert!(!verify_password("wrong horse battery staple", &hash).unwrap());
    }

    #[test]
    fn the_same_password_hashes_differently_each_time() {
        // Distinct salts, so two users with the same password are not
        // visibly identical in the users table.
        let first = hash_password("correct horse battery staple").unwrap();
        let second = hash_password("correct horse battery staple").unwrap();
        assert_ne!(first, second);
    }

    #[test]
    fn a_corrupt_stored_hash_is_an_error_not_a_silent_false() {
        assert!(matches!(
            verify_password("whatever", "not-a-phc-string"),
            Err(PasswordError::MalformedHash)
        ));
    }

    #[test]
    fn short_passwords_are_refused() {
        assert!(validate_password("short").is_err());
        assert!(validate_password("exactly12chr").is_ok());
    }

    #[test]
    fn email_normalisation_trims_and_validates() {
        assert_eq!(normalize_email("  a@b.com  ").unwrap(), "a@b.com");
        for bad in ["", "no-at-sign", "@b.com", "a@", "a@nodot"] {
            assert!(normalize_email(bad).is_err(), "should reject {bad:?}");
        }
    }

    #[test]
    fn session_tokens_are_unique_and_stored_only_as_hashes() {
        let first = generate_session_token();
        let second = generate_session_token();
        assert_ne!(first.raw, second.raw);
        assert_ne!(first.hash, second.hash);
        // 32 random bytes rendered as hex.
        assert_eq!(first.raw.len(), 64);
        assert_eq!(hash_session_token(&first.raw), first.hash);
    }

    #[test]
    fn only_active_accounts_may_sign_in() {
        assert!(Status::Active.may_sign_in());
        assert!(!Status::Pending.may_sign_in());
        assert!(!Status::Suspended.may_sign_in());
    }

    #[test]
    fn roles_and_statuses_round_trip_through_their_strings() {
        for role in [Role::Admin, Role::User] {
            assert_eq!(Role::parse(role.as_str()), Some(role));
        }
        for status in [Status::Pending, Status::Active, Status::Suspended] {
            assert_eq!(Status::parse(status.as_str()), Some(status));
        }
        assert_eq!(Role::parse("superuser"), None);
    }
}
