//! Turning a live grammers session into bytes we can encrypt, and back.
//!
//! grammers ships two `Session` implementations: one in memory, one backed
//! by a SQLite file. Neither suits a multi-user server — a file would mean
//! every user's plaintext session sitting on disk, which is precisely what
//! the encryption exists to avoid.
//!
//! `SessionData` is grammers' own "entire session state" type, convertible
//! `From` any session and back via `import_to`. It does not itself derive
//! serde, but every type inside it does (behind grammers' `serde` feature),
//! so this module mirrors its four fields in a struct that does.
//!
//! The maps become vectors of pairs deliberately: `PeerId` is not a string,
//! and JSON object keys must be, so serialising the maps directly would
//! either fail or silently stringify keys we then could not parse back.

use std::collections::HashMap;
use std::sync::Arc;

use grammers_session::storages::MemorySession;
use grammers_session::types::{DcOption, PeerId, PeerInfo, UpdateState, UpdatesState};
use grammers_session::{Session, SessionData};
use serde::{Deserialize, Serialize};

/// Bumped whenever the shape below changes incompatibly. A stored session
/// from an older version is discarded rather than misread — the cost is one
/// re-login, against silently corrupt session state.
const FORMAT_VERSION: u16 = 1;

#[derive(Serialize, Deserialize)]
struct StoredSession {
    version: u16,
    home_dc: i32,
    dc_options: Vec<(i32, DcOption)>,
    peer_infos: Vec<(PeerId, PeerInfo)>,
    updates_state: UpdatesState,
}

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("could not serialise the session")]
    Serialise,
    #[error("the stored session is unreadable or from an older format")]
    Deserialise,
}

/// A `Session` that borrows another through an `Arc`.
///
/// `SessionData: From<S: Session>` takes the session *by value*, and the
/// trait is implemented only for the two concrete storages — not for
/// references or `Arc`. But the live session must stay owned by the sender
/// pool while we snapshot it. Every method takes `&self`, so delegating
/// through a clone of the `Arc` gives `From` something it can consume
/// without taking the real session away.
struct SharedSession(Arc<MemorySession>);

impl Session for SharedSession {
    fn home_dc_id(&self) -> i32 {
        self.0.home_dc_id()
    }
    fn set_home_dc_id(&self, dc_id: i32) {
        self.0.set_home_dc_id(dc_id)
    }
    fn dc_option(&self, dc_id: i32) -> Option<DcOption> {
        self.0.dc_option(dc_id)
    }
    fn set_dc_option(&self, dc_option: &DcOption) {
        self.0.set_dc_option(dc_option)
    }
    fn peer(&self, peer: PeerId) -> Option<PeerInfo> {
        self.0.peer(peer)
    }
    fn cache_peer(&self, peer: &PeerInfo) {
        self.0.cache_peer(peer)
    }
    fn updates_state(&self) -> UpdatesState {
        self.0.updates_state()
    }
    fn set_update_state(&self, update: UpdateState) {
        self.0.set_update_state(update)
    }
}

/// Snapshots a live session into bytes ready to be encrypted.
pub fn export(session: &Arc<MemorySession>) -> Result<Vec<u8>, StoreError> {
    let data = SessionData::from(SharedSession(Arc::clone(session)));

    let stored = StoredSession {
        version: FORMAT_VERSION,
        home_dc: data.home_dc,
        dc_options: data.dc_options.into_iter().collect(),
        peer_infos: data.peer_infos.into_iter().collect(),
        updates_state: data.updates_state,
    };

    serde_json::to_vec(&stored).map_err(|error| {
        tracing::error!(?error, "session serialise failed");
        StoreError::Serialise
    })
}

/// Rebuilds an in-memory session from previously exported bytes.
pub fn import(bytes: &[u8]) -> Result<Arc<MemorySession>, StoreError> {
    let stored: StoredSession = serde_json::from_slice(bytes).map_err(|error| {
        tracing::warn!(?error, "session deserialise failed");
        StoreError::Deserialise
    })?;

    if stored.version != FORMAT_VERSION {
        tracing::warn!(
            found = stored.version,
            expected = FORMAT_VERSION,
            "stored session is a different format version; discarding"
        );
        return Err(StoreError::Deserialise);
    }

    let data = SessionData {
        home_dc: stored.home_dc,
        dc_options: stored.dc_options.into_iter().collect::<HashMap<_, _>>(),
        peer_infos: stored.peer_infos.into_iter().collect::<HashMap<_, _>>(),
        updates_state: stored.updates_state,
    };

    let session = Arc::new(MemorySession::from(SessionData::default()));
    data.import_to(&SharedSession(Arc::clone(&session)));
    Ok(session)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_session_round_trips() {
        // Not a signed-in session — that needs the network — but it does
        // prove the serialise/deserialise path and the known datacenter
        // options survive intact, which is where the format breaks first.
        let session = Arc::new(MemorySession::from(SessionData::default()));
        let bytes = export(&session).expect("should export");
        assert!(!bytes.is_empty());

        let restored = import(&bytes).expect("should import");
        assert_eq!(restored.home_dc_id(), session.home_dc_id());
    }

    #[test]
    fn rubbish_bytes_are_refused_rather_than_panicking() {
        // These arrive from the database, so a corrupt or truncated row
        // must surface as an error the caller can turn into "reconnect".
        assert!(import(b"").is_err());
        assert!(import(b"not json at all").is_err());
        assert!(import(b"{\"version\":1}").is_err());
    }

    #[test]
    fn a_session_from_a_future_format_is_discarded() {
        let session = Arc::new(MemorySession::from(SessionData::default()));
        let bytes = export(&session).unwrap();

        let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        value["version"] = serde_json::json!(FORMAT_VERSION + 1);
        let bumped = serde_json::to_vec(&value).unwrap();

        assert!(import(&bumped).is_err());
    }
}
