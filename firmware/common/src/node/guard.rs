//! Guards around the encrypted sessions.
//!
//! * **A packet that does not fit a session proves nothing**: anyone can make
//!   one look like it comes from a contact. So the session is first tested
//!   with `SESSION_PROBE`; a contact that can read it acknowledges it, and
//!   the session stays. Only when a probe has gone unanswered for
//!   [`PROBE_WAIT`] and another unreadable packet comes is a new session made.
//! * **Replays**: the packets opened lately are remembered. One that comes
//!   again is acknowledged again (the first acknowledgement may have been
//!   lost) but not opened. Packets that start a session, or need none, are
//!   also stored in flash, so that after a restart an old one cannot set up a
//!   stale session.

use super::{FwNode, Settings, log};
use embassy_time::{Duration, Instant};
use maarse_core::Address;
use maarse_core::content::{self, person as op};
use maarse_core::router::Proof;
use rand_core::{CryptoRng, RngCore};

/// How long a probe waits for its acknowledgement.
const PROBE_WAIT: Duration = Duration::from_secs(60);
/// An unanswered probe older than this says nothing any more.
const PROBE_LIFETIME: Duration = Duration::from_secs(3600);
/// Packets remembered (also the number read back from flash at start).
const SEEN: usize = 24;

struct Suspect {
    peer: Address,
    since: Instant,
    msg_id: u64,
}

#[derive(Default)]
pub(super) struct Guard {
    suspects: heapless::Vec<Suspect, 4>,
    /// Packet ID and the proof we acknowledged it with (zeros: none).
    seen: heapless::Deque<(u64, [u8; 8]), SEEN>,
}

impl Guard {
    /// A packet stored in flash, read at start (oldest first).
    pub(super) fn restore(&mut self, rid: u64, proof: [u8; 8]) {
        if self.seen.is_full() {
            self.seen.pop_front();
        }
        let _ = self.seen.push_back((rid, proof));
    }

    /// Something from `peer` opened: its session works.
    pub(super) fn opened(&mut self, peer: Address) {
        self.suspects.retain(|s| s.peer != peer);
    }

}

impl<R: RngCore + CryptoRng, S: Settings> FwNode<R, S> {
    /// Is `msg_id` a probe? The session then works.
    pub(super) fn probe_acked(&mut self, msg_id: u64) -> bool {
        let Some(i) = self.guard.suspects.iter().position(|s| s.msg_id == msg_id) else { return false };
        self.guard.suspects.swap_remove(i);
        log("The session works: the unreadable packet is ignored");
        true
    }

    /// If this packet was opened before: acknowledges it again and returns
    /// `true`, so that it is not opened a second time.
    #[inline(never)]
    pub(super) fn replayed(&mut self, rid: u64) -> bool {
        let Some(proof) = self.guard.seen.iter().find(|s| s.0 == rid).map(|s| s.1) else { return false };
        log("Message received before: not shown again");
        self.acknowledge(rid, (proof != [0; 8]).then_some(proof));
        true
    }

    /// Remembers an opened packet; `keep`: also across a restart.
    pub(super) fn remember(&mut self, rid: u64, proof: Option<Proof>, keep: bool) {
        let proof = proof.unwrap_or([0; 8]);
        self.guard.restore(rid, proof);
        if keep {
            self.settings.replay_add(rid, &proof);
        }
    }

    /// A packet from `peer` did not fit our session. `true`: the session is
    /// really broken, replace it. Otherwise it is tested (or still being
    /// tested) and kept.
    #[inline(never)]
    pub(super) fn session_is_lost(&mut self, peer: Address) -> bool {
        let age = self.guard.suspects.iter().find(|s| s.peer == peer).map(|s| s.since.elapsed());
        match age {
            Some(a) if a < PROBE_WAIT => false,
            Some(a) if a < PROBE_LIFETIME => {
                self.guard.opened(peer);
                true
            }
            _ => {
                if let Ok(msg_id) = self.send_plain(peer, &[content::PERSON, op::SESSION_PROBE]) {
                    self.guard.suspects.retain(|s| s.peer != peer);
                    if self.guard.suspects.is_full() {
                        self.guard.suspects.remove(0);
                    }
                    let _ = self.guard.suspects.push(Suspect { peer, since: Instant::now(), msg_id });
                    log("Unreadable message: testing the session");
                }
                false
            }
        }
    }
}
