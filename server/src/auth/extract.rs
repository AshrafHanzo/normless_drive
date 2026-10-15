//! Request extractors.
//!
//! `CurrentUser` as a handler argument is the only way a route gets a user
//! id, and it is the single place session validity and account status are
//! checked. A handler that does not take it is unauthenticated by
//! construction, which is a much easier property to audit than remembering
//! to call a guard at the top of every function.

use actix_web::{dev::Payload, web, FromRequest, HttpRequest};
use std::future::Future;
use std::pin::Pin;
use uuid::Uuid;

use super::{hash_session_token, CurrentUser, Role, Status, SESSION_COOKIE};
use crate::{db::Db, error::ApiError};

impl FromRequest for CurrentUser {
    type Error = ApiError;
    type Future = Pin<Box<dyn Future<Output = Result<Self, Self::Error>>>>;

    fn from_request(request: &HttpRequest, _payload: &mut Payload) -> Self::Future {
        let cookie = request.cookie(SESSION_COOKIE).map(|c| c.value().to_owned());
        let pool = request.app_data::<web::Data<Db>>().cloned();

        Box::pin(async move {
            let pool = pool.ok_or_else(|| {
                ApiError::Internal(anyhow::anyhow!("database pool missing from app data"))
            })?;
            let cookie = cookie.ok_or(ApiError::Unauthenticated)?;
            if cookie.is_empty() {
                return Err(ApiError::Unauthenticated);
            }

            let token_hash = hash_session_token(&cookie);

            let client = pool
                .get()
                .await
                .map_err(|error| ApiError::Internal(error.into()))?;

            // Expiry is filtered in SQL rather than compared in Rust so an
            // expired session is indistinguishable from a missing one, and
            // there is no window where a stale row is treated as valid.
            let found = client
                .query_opt(
                    "SELECT u.id, u.email, u.role, u.status
                     FROM app_sessions s
                     JOIN users u ON u.id = s.user_id
                     WHERE s.token_hash = $1 AND s.expires_at > now()",
                    &[&token_hash],
                )
                .await
                .map_err(|error| ApiError::Internal(error.into()))?;

            let row = found.ok_or(ApiError::Unauthenticated)?;
            let id: Uuid = row.get("id");
            let email: String = row.get("email");
            let role: String = row.get("role");
            let status: String = row.get("status");

            // Re-checked on every request, not just at login: suspending an
            // account has to take effect immediately, not whenever their
            // existing session happens to expire.
            let status = Status::parse(&status).unwrap_or(Status::Suspended);
            if !status.may_sign_in() {
                return Err(ApiError::Forbidden(
                    "Your account is not currently active.".into(),
                ));
            }

            Ok(CurrentUser {
                id,
                email,
                role: Role::parse(&role).unwrap_or(Role::User),
            })
        })
    }
}

/// An authenticated user who is also an administrator.
///
/// A separate type rather than a boolean check inside handlers, so an admin
/// route cannot be written that forgets to check.
pub struct AdminUser(pub CurrentUser);

impl FromRequest for AdminUser {
    type Error = ApiError;
    type Future = Pin<Box<dyn Future<Output = Result<Self, Self::Error>>>>;

    fn from_request(request: &HttpRequest, payload: &mut Payload) -> Self::Future {
        let inner = CurrentUser::from_request(request, payload);
        Box::pin(async move {
            let user = inner.await?;
            if !user.is_admin() {
                // 403 with a flat message: whether a given route exists is
                // not something a non-admin needs confirmed.
                return Err(ApiError::Forbidden("Administrator access required.".into()));
            }
            Ok(AdminUser(user))
        })
    }
}
