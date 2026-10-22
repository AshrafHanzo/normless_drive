//! What each account is allowed to do, decided by an administrator.
//!
//! Checked on the server for every operation. The UI also hides what a user
//! cannot do, but that is a convenience — hiding a button is not a
//! permission check, and anyone can call the endpoint directly.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    db::Db,
    error::{ApiError, ApiResult},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capability {
    Upload,
    Download,
    Delete,
    CreateFolders,
    Share,
}

impl Capability {
    fn column(self) -> &'static str {
        match self {
            Capability::Upload => "can_upload",
            Capability::Download => "can_download",
            Capability::Delete => "can_delete",
            Capability::CreateFolders => "can_create_folders",
            Capability::Share => "can_share",
        }
    }

    /// Phrased for the person who hit the wall, not for the developer.
    fn refusal(self) -> &'static str {
        match self {
            Capability::Upload => "Your account is not allowed to upload files.",
            Capability::Download => "Your account is not allowed to download files.",
            Capability::Delete => "Your account is not allowed to delete files.",
            Capability::CreateFolders => "Your account is not allowed to create folders.",
            Capability::Share => "Your account is not allowed to create share links.",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Permissions {
    pub can_upload: bool,
    pub can_download: bool,
    pub can_delete: bool,
    pub can_create_folders: bool,
    pub can_share: bool,
    /// Bytes. Zero means no cap.
    pub storage_quota_bytes: i64,
}

impl Default for Permissions {
    fn default() -> Self {
        Self {
            can_upload: true,
            can_download: true,
            can_delete: true,
            can_create_folders: true,
            can_share: true,
            storage_quota_bytes: 0,
        }
    }
}

pub async fn load(pool: &Db, user: Uuid) -> ApiResult<Permissions> {
    let client = pool.get().await?;
    let row = client
        .query_opt(
            "SELECT can_upload, can_download, can_delete, can_create_folders,
                    can_share, storage_quota_bytes
               FROM user_permissions WHERE user_id = $1",
            &[&user],
        )
        .await?;

    // A missing row means the account predates this table or was created in
    // a race. Permissive rather than locked out: an admin can always
    // withdraw, but a user who cannot do anything and sees no explanation
    // just files a bug.
    Ok(match row {
        Some(row) => Permissions {
            can_upload: row.get("can_upload"),
            can_download: row.get("can_download"),
            can_delete: row.get("can_delete"),
            can_create_folders: row.get("can_create_folders"),
            can_share: row.get("can_share"),
            storage_quota_bytes: row.get("storage_quota_bytes"),
        },
        None => Permissions::default(),
    })
}

/// Refuses the request unless the user holds this capability.
pub async fn require(pool: &Db, user: Uuid, capability: Capability) -> ApiResult<()> {
    let client = pool.get().await?;
    let row = client
        .query_opt(
            &format!(
                // The column name comes from the Capability enum, never from
                // user input, so this cannot be injected — there is no way
                // to reach it with an arbitrary string.
                "SELECT {} AS allowed FROM user_permissions WHERE user_id = $1",
                capability.column()
            ),
            &[&user],
        )
        .await?;

    let allowed: bool = row.map(|row| row.get("allowed")).unwrap_or(true);
    if allowed {
        Ok(())
    } else {
        Err(ApiError::Forbidden(capability.refusal().to_string()))
    }
}

/// Current usage against the quota, in bytes.
pub async fn usage(pool: &Db, user: Uuid) -> ApiResult<i64> {
    let client = pool.get().await?;
    let row = client
        .query_one(
            "SELECT COALESCE(sum(size_bytes), 0)::bigint AS used
               FROM uploads WHERE user_id = $1",
            &[&user],
        )
        .await?;
    Ok(row.get("used"))
}

/// Refuses an upload that would take the account past its quota.
pub async fn check_quota(pool: &Db, user: Uuid, incoming: i64) -> ApiResult<()> {
    let limits = load(pool, user).await?;
    if limits.storage_quota_bytes == 0 {
        return Ok(());
    }

    let used = usage(pool, user).await?;
    if used + incoming > limits.storage_quota_bytes {
        return Err(ApiError::Forbidden(format!(
            "That would put you over your {} storage limit. You have used {}.",
            human(limits.storage_quota_bytes),
            human(used)
        )));
    }
    Ok(())
}

fn human(bytes: i64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_capability_maps_to_a_distinct_column() {
        let all = [
            Capability::Upload,
            Capability::Download,
            Capability::Delete,
            Capability::CreateFolders,
            Capability::Share,
        ];
        let mut columns: Vec<&str> = all.iter().map(|c| c.column()).collect();
        columns.sort_unstable();
        let before = columns.len();
        columns.dedup();
        assert_eq!(columns.len(), before, "two capabilities share a column");
    }

    #[test]
    fn column_names_are_safe_to_interpolate() {
        // They are formatted into SQL, so they must stay plain identifiers
        // even if someone adds a capability later.
        for capability in [
            Capability::Upload,
            Capability::Download,
            Capability::Delete,
            Capability::CreateFolders,
            Capability::Share,
        ] {
            let column = capability.column();
            assert!(
                column.chars().all(|c| c.is_ascii_lowercase() || c == '_'),
                "{column} is not a plain identifier"
            );
        }
    }

    #[test]
    fn a_refusal_tells_the_user_what_happened() {
        for capability in [Capability::Upload, Capability::Share] {
            let text = capability.refusal();
            assert!(text.starts_with("Your account"));
            assert!(text.ends_with('.'));
        }
    }

    #[test]
    fn sizes_read_sensibly() {
        assert_eq!(human(512), "512 B");
        assert_eq!(human(1536), "1.5 KB");
        assert_eq!(human(5 * 1024 * 1024 * 1024), "5.0 GB");
    }
}
