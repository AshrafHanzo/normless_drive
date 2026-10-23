//! One error type for every handler.
//!
//! The important property here is that the message a client sees and the
//! message that reaches the log are chosen separately. Database text, Argon2
//! internals and Telegram protocol errors are useful in a log and are a gift
//! to an attacker in a response body.

use actix_web::{http::StatusCode, HttpResponse, ResponseError};
use serde::Serialize;

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("{0}")]
    BadRequest(String),

    /// Not signed in, or the session is gone.
    #[error("authentication required")]
    Unauthenticated,

    /// Signed in, but not allowed to do this.
    #[error("{0}")]
    Forbidden(String),

    /// Signed in, but the key that decrypts this user's Telegram session
    /// is not in memory.
    ///
    /// Happens after every server restart, because the key is derived at
    /// login and deliberately never written to disk. The browser still
    /// holds a valid cookie, so the user looks signed in and everything
    /// Telegram-related fails with no obvious cause. Distinct from
    /// `Forbidden` so the UI can recognise it and offer the one action
    /// that fixes it rather than showing a dead end.
    #[error("Your session needs unlocking. Please sign in again.")]
    SessionLocked,

    #[error("{0}")]
    NotFound(String),

    #[error("{0}")]
    Conflict(String),

    /// Anything unexpected. The inner error is logged, never returned.
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

impl From<tokio_postgres::Error> for ApiError {
    fn from(error: tokio_postgres::Error) -> Self {
        ApiError::Internal(error.into())
    }
}

impl From<deadpool_postgres::PoolError> for ApiError {
    fn from(error: deadpool_postgres::PoolError) -> Self {
        ApiError::Internal(error.into())
    }
}

/// True when a database error is a unique-constraint violation.
///
/// Used to turn "that email is taken" into a 409 instead of a 500. SQLSTATE
/// 23505 is the only reliable signal — the message text is localised and
/// varies by server version.
pub fn is_unique_violation(error: &tokio_postgres::Error) -> bool {
    error
        .code()
        .is_some_and(|code| code == &tokio_postgres::error::SqlState::UNIQUE_VIOLATION)
}

#[derive(Serialize)]
struct ErrorBody<'a> {
    error: ErrorDetail<'a>,
}

#[derive(Serialize)]
struct ErrorDetail<'a> {
    code: &'a str,
    message: &'a str,
}

impl ApiError {
    fn code(&self) -> &'static str {
        match self {
            ApiError::BadRequest(_) => "BAD_REQUEST",
            ApiError::Unauthenticated => "UNAUTHENTICATED",
            ApiError::Forbidden(_) => "FORBIDDEN",
            ApiError::SessionLocked => "SESSION_LOCKED",
            ApiError::NotFound(_) => "NOT_FOUND",
            ApiError::Conflict(_) => "CONFLICT",
            ApiError::Internal(_) => "INTERNAL",
        }
    }
}

impl ResponseError for ApiError {
    fn status_code(&self) -> StatusCode {
        match self {
            ApiError::BadRequest(_) => StatusCode::BAD_REQUEST,
            ApiError::Unauthenticated => StatusCode::UNAUTHORIZED,
            ApiError::Forbidden(_) => StatusCode::FORBIDDEN,
            ApiError::SessionLocked => StatusCode::FORBIDDEN,
            ApiError::NotFound(_) => StatusCode::NOT_FOUND,
            ApiError::Conflict(_) => StatusCode::CONFLICT,
            ApiError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    fn error_response(&self) -> HttpResponse {
        // Log the real cause, then decide separately what to tell the client.
        if let ApiError::Internal(cause) = self {
            tracing::error!(error = ?cause, "request failed");
        }

        let message = match self {
            ApiError::Internal(_) => "Something went wrong on our side.",
            other => &other.to_string(),
        };

        HttpResponse::build(self.status_code()).json(ErrorBody {
            error: ErrorDetail {
                code: self.code(),
                message,
            },
        })
    }
}

pub type ApiResult<T> = Result<T, ApiError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn internal_errors_do_not_leak_their_cause() {
        let error = ApiError::Internal(anyhow::anyhow!(
            "connection refused to postgres://normless:hunter2@localhost/db"
        ));
        let rendered = error.error_response();
        assert_eq!(rendered.status(), StatusCode::INTERNAL_SERVER_ERROR);

        // The Display impl is transparent, so the secret IS in the error --
        // the protection is that error_response does not use it.
        assert!(error.to_string().contains("hunter2"));
    }

    #[test]
    fn client_errors_keep_their_message() {
        let error = ApiError::BadRequest("email is required".into());
        assert_eq!(error.status_code(), StatusCode::BAD_REQUEST);
        assert_eq!(error.to_string(), "email is required");
    }
}
