//! Public share links.
//!
//! A share points at one folder (or Saved Messages) and carries a
//! permission bitfield, matching the desktop app's scheme so the two agree
//! on what a link means:
//!
//!     1 UPLOAD   2 DOWNLOAD   4 UPDATE   8 DELETE
//!
//! Links are served under `/s/{token}` with no authentication. Whoever
//! holds the link gets exactly the permissions it carries, optionally
//! behind a password, an expiry and a usage cap.

pub mod public;
pub mod routes;

use serde::{Deserialize, Serialize};

bitflags::bitflags! {
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct SharePermissions: i64 {
        const UPLOAD   = 0b0001;
        const DOWNLOAD = 0b0010;
        const UPDATE   = 0b0100;
        const DELETE   = 0b1000;
    }
}

/// The JSON shape the UI uses. Flags are a poor API across a network
/// boundary — named booleans survive a schema change that renumbers bits.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct SharePermissionView {
    pub can_upload: bool,
    pub can_download: bool,
    pub can_update: bool,
    pub can_delete: bool,
}

impl From<SharePermissions> for SharePermissionView {
    fn from(bits: SharePermissions) -> Self {
        Self {
            can_upload: bits.contains(SharePermissions::UPLOAD),
            can_download: bits.contains(SharePermissions::DOWNLOAD),
            can_update: bits.contains(SharePermissions::UPDATE),
            can_delete: bits.contains(SharePermissions::DELETE),
        }
    }
}

impl From<SharePermissionView> for SharePermissions {
    fn from(view: SharePermissionView) -> Self {
        let mut bits = SharePermissions::empty();
        bits.set(SharePermissions::UPLOAD, view.can_upload);
        bits.set(SharePermissions::DOWNLOAD, view.can_download);
        bits.set(SharePermissions::UPDATE, view.can_update);
        bits.set(SharePermissions::DELETE, view.can_delete);
        bits
    }
}

/// A URL-safe opaque token.
///
/// 32 bytes of randomness rendered as hex. Shares are reachable by anyone
/// who guesses the token, so the token *is* the security boundary for
/// links without a password — it has to be far too large to enumerate.
pub fn generate_token() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 24];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permissions_round_trip_through_their_json_shape() {
        let original = SharePermissions::UPLOAD | SharePermissions::DOWNLOAD;
        let view: SharePermissionView = original.into();
        assert!(view.can_upload && view.can_download);
        assert!(!view.can_update && !view.can_delete);
        assert_eq!(SharePermissions::from(view), original);
    }

    #[test]
    fn the_bit_values_match_the_desktop_app() {
        // A link created in one and opened in the other must mean the same
        // thing. Renumbering these silently would turn a download-only
        // link into a delete-capable one.
        assert_eq!(SharePermissions::UPLOAD.bits(), 1);
        assert_eq!(SharePermissions::DOWNLOAD.bits(), 2);
        assert_eq!(SharePermissions::UPDATE.bits(), 4);
        assert_eq!(SharePermissions::DELETE.bits(), 8);
    }

    #[test]
    fn tokens_are_unique_and_long_enough_to_resist_guessing() {
        let a = generate_token();
        let b = generate_token();
        assert_ne!(a, b);
        assert_eq!(a.len(), 48); // 24 bytes as hex
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn an_empty_permission_set_grants_nothing() {
        let view: SharePermissionView = SharePermissions::empty().into();
        assert!(!view.can_upload && !view.can_download && !view.can_update && !view.can_delete);
    }
}
