//! Where each persistent host session's event stream stood when this Core
//! last had a connection to it, so a reconnection continues it (design §9.3).
//!
//! A connection that resumes a session asks the host for the events after
//! the last one the previous connection received. The previous connection's
//! subscribers already showed everything up to there, and the events after it
//! wait in the new connection until a run window attaches to its process
//! (`remote.attachProcess`). Without a remembered position, for example after
//! the application restarted, the host replays every event it still keeps:
//! the attach then reports that output may be missing.
//!
//! The positions live only in this process's memory, keyed by the session
//! key. They hold no host details and are bounded to the most recent
//! sessions.

use lithe_remote::client::session::EventPosition;
use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};

/// Sessions whose position is remembered; the oldest is forgotten first.
const REMEMBERED_SESSIONS: usize = 32;

/// The connection that last owned a session.
struct Remembered {
    key: String,
    position: EventPosition,
    /// Whether that connection had every event of the resources earlier
    /// connections created, or lost some of them.
    complete: bool,
}

/// Where a reconnection continues the session's event stream.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Continuation {
    /// Sequence number of the latest event the previous connection received.
    pub(crate) after_seq: u64,
    /// Whether the previous connection lost no event of a resource that an
    /// attach can still receive.
    pub(crate) complete: bool,
}

fn positions() -> &'static Mutex<VecDeque<Remembered>> {
    static POSITIONS: OnceLock<Mutex<VecDeque<Remembered>>> = OnceLock::new();
    POSITIONS.get_or_init(Mutex::default)
}

/// Where the previous connection to session `key` stopped, if this Core had
/// one.
pub(crate) fn continuation(key: &str) -> Option<Continuation> {
    positions()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .iter()
        .find(|remembered| remembered.key == key)
        .map(|remembered| Continuation {
            after_seq: remembered.position.seq(),
            // Events the connection kept for a process nobody attached to
            // were acknowledged and are gone with it.
            complete: remembered.complete && !remembered.position.discarded(),
        })
}

/// Records the connection that now owns session `key`, replacing the
/// previous one.
pub(crate) fn remember(key: &str, position: EventPosition, complete: bool) {
    let mut positions = positions()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    positions.retain(|remembered| remembered.key != key);
    if positions.len() >= REMEMBERED_SESSIONS {
        positions.pop_front();
    }
    positions.push_back(Remembered {
        key: key.to_owned(),
        position,
        complete,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_session_continues_where_its_latest_connection_stopped() {
        let key = "resumption-latest";
        assert_eq!(continuation(key), None);
        remember(key, EventPosition::default(), false);
        remember(key, EventPosition::default(), true);
        assert_eq!(
            continuation(key),
            Some(Continuation {
                after_seq: 0,
                complete: true
            })
        );
    }

    #[test]
    fn only_the_most_recent_sessions_are_remembered() {
        for index in 0..=REMEMBERED_SESSIONS {
            remember(
                &format!("resumption-bounded-{index}"),
                EventPosition::default(),
                true,
            );
        }
        assert_eq!(continuation("resumption-bounded-0"), None);
        assert!(continuation(&format!("resumption-bounded-{REMEMBERED_SESSIONS}")).is_some());
        assert!(
            positions()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .len()
                <= REMEMBERED_SESSIONS
        );
    }
}
