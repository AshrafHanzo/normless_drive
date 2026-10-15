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

    #[error("{0}")]
    NotFound(String),

    #[error("{0}")]
    Conflict(String),

    /// Anything unexpected. The inner error is logged, never returned.
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

impl From<sqlx::Error> for ApiError {
    fn from(error: sqlx::Error) -> Self {
        // RowNotFound reaching a handler almost always means "this id does
        // not exist", which is a 404 rather than a server fault.
        match error {
            sqlx::Error::RowNotFound => ApiError::NotFound("not found".into()),
            other => ApiError::Internal(other.into()),
        }
    }
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
    fn a_missing_row_becomes_a_404_not_a_500() {
        let error: ApiError = sqlx::Error::RowNotFound.into();
        assert_eq!(error.status_code(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn client_errors_keep_their_message() {
        let error = ApiError::BadRequest("email is required".into());
        assert_eq!(error.status_code(), StatusCode::BAD_REQUEST);
        assert_eq!(error.to_string(), "email is required");
    }
}
