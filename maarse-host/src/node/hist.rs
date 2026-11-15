//! Hooks between the node and the message history (see history.rs): store messages,
//! answer 0x30-0x32 and send our own messages live to the other screens.

use super::{Node, err_ev, unix_now};
use crate::ClientId;
use crate::history::{self, HistCmd, History, Item};
use maarse_core::Address;
use maarse_core::companion::{Event, error_code};
use std::time::{Duration, Instant};

pub(super) struct Hist {
    pub(super) store: Option<History>,
    last_prune: Instant,
}

impl Hist {
    pub(super) fn open(data_dir: &std::path::Path, days: Option<u32>) -> Self {
        let store = days.and_then(|d| match History::open(data_dir.join("history"), d, unix_now()) {
            Ok(h) => Some(h),
            Err(e) => {
                eprintln!("history could not be opened: {e}");
                None
            }
        });
        Hist { store, last_prune: Instant::now() }
    }
}

impl Node {
    fn send_raw(&self, id: ClientId, f: Vec<u8>) {
        if let Some(tx) = self.clients.get(&id) {
            let _ = tx.send(f);
        }
    }

    fn broadcast_raw(&self, f: &[u8], except: Option<ClientId>) {
        for (cid, tx) in &self.clients {
            if Some(*cid) != except {
                let _ = tx.send(f.to_vec());
            }
        }
    }

    /// On connect: announce that history exists, and send unread counts.
    pub(super) fn hist_greet(&self, id: ClientId) {
        let Some(h) = &self.hist.store else { return };
        self.send_raw(id, h.info_frame());
        for (c, mark, unread) in h.read_state() {
            self.send_raw(id, history::read_mark_frame(c, mark, unread));
        }
    }

    /// Called for every event that is sent to all clients.
    pub(super) fn hist_observe(&mut self, ev: &Event) {
        let Some(h) = self.hist.store.as_mut() else { return };
        let me = self.addr;
        let base = |kind, conv, from, hops, content: &[u8]| Item {
            id: 0,
            ts: unix_now(),
            kind,
            outgoing: false,
            state: history::ST_RECEIVED,
            hops,
            msg_id: 0,
            conv,
            from,
            content: content.to_vec(),
        };
        match ev {
            Event::DirectReceived { from, hops, text } => {
                h.add(base(history::KIND_DIRECT, *from, *from, *hops, text));
            }
            Event::ChannelReceived { channel, from, hops, text } if *from != me => {
                h.add(base(history::KIND_CHANNEL, *channel, *from, *hops, text));
            }
            Event::FileIncoming { from, size, mime, name, .. } => {
                h.add(base(history::KIND_FILE, *from, *from, 0, &history::file_content(*size, mime, name)));
            }
            Event::Acked { msg_id } => {
                h.set_state(*msg_id, history::ST_ACKED);
            }
            Event::CarryAcked { msg_id } => {
                h.set_state(*msg_id, history::ST_CARRIED);
            }
            Event::Deposited { msg_id } => {
                h.set_state(*msg_id, history::ST_DEPOSITED);
            }
            _ => {}
        }
    }

    /// One of our screens (`id`) has sent something: store it and show it live
    /// on the other screens.
    pub(super) fn hist_sent(&mut self, id: ClientId, kind: u8, conv: Address, msg_id: u64, content: &[u8]) {
        let from = self.addr;
        let Some(h) = self.hist.store.as_mut() else { return };
        let it = h.add(Item {
            id: 0,
            ts: unix_now(),
            kind,
            outgoing: true,
            state: history::ST_SENT,
            hops: 0,
            msg_id,
            conv,
            from,
            content: content.to_vec(),
        });
        self.broadcast_raw(&history::item_frame(&it, true), Some(id));
    }

    pub(super) fn hist_command(&mut self, id: ClientId, frame: &[u8]) {
        let Some(h) = self.hist.store.as_mut() else {
            let req = if frame.first() == Some(&history::CMD_GET_HISTORY) && frame.len() >= 3 {
                u16::from_le_bytes([frame[1], frame[2]])
            } else {
                0
            };
            return self.send_client(id, &err_ev(req, error_code::BAD_FRAME, "History is disabled"));
        };
        match history::parse_command(frame) {
            Some(HistCmd::Get { req, conv, before, limit }) => {
                let (items, more) = h.page(conv, before, limit as usize);
                let frames: Vec<Vec<u8>> = items.iter().map(|i| history::item_frame(i, false)).collect();
                let oldest = items.first().map_or(0, |i| i.id);
                let n = frames.len() as u8;
                for f in frames {
                    self.send_raw(id, f);
                }
                self.send_raw(id, history::end_frame(req, conv, n, more, oldest));
            }
            Some(HistCmd::MarkRead { conv, up_to }) => {
                let (mark, unread) = h.mark_read(conv, up_to);
                self.broadcast_raw(&history::read_mark_frame(conv, mark, unread), None);
            }
            Some(HistCmd::GetReadState) => {
                let frames: Vec<Vec<u8>> =
                    h.read_state().into_iter().map(|(c, m, u)| history::read_mark_frame(c, m, u)).collect();
                for f in frames {
                    self.send_raw(id, f);
                }
            }
            None => self.send_client(id, &err_ev(0, error_code::BAD_FRAME, "invalid frame")),
        }
    }

    /// From housekeeping: remove old records about every hour.
    pub(super) fn hist_maintain(&mut self) {
        if self.hist.last_prune.elapsed() < Duration::from_secs(3600) {
            return;
        }
        self.hist.last_prune = Instant::now();
        if let Some(h) = self.hist.store.as_mut()
            && let Ok(n) = h.prune(unix_now())
            && n > 0
        {
            self.log(format!("history: {n} old messages deleted"));
        }
    }
}
