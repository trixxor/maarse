//! Post offices: store-and-forward mail for nodes that are offline.
//!
//! The model, after the postal service, SMS centres and Reticulum's LXMF
//! propagation nodes:
//!
//! * **Post offices** are nodes with storage: hosts (class 3, a disk),
//!   repeaters with megabytes of flash (class 2) or a little flash (class 1).
//!   They say so in their hellos and announces (role flags bits 4-5).
//! * **A sender drops mail at its own nearest post office** (a `Deposit`
//!   with [`OP_STORE`]) when the recipient has not acknowledged a message
//!   within 45 s, like posting a letter in the nearest post box.
//! * **Hosts keep copies on three hosts** chosen from the recipient's address
//!   by rendezvous hashing ([`replicas`]), over TCP, so anyone can work out
//!   where a node's mail is, wherever the node turns up.
//! * **The recipient asks for its mail** with a [`MailSync`](crate::packet::PacketType::MailSync)
//!   (a 2-hop flood) when it starts, regains coverage, sees its ID in a post
//!   office's hello ("mail waiting", like the TIM in Wi-Fi beacons) or every
//!   few hours. A post office that hears it delivers what it holds and, if it
//!   is a host, fetches the copies from the replicas over TCP first
//!   ([`OP_FETCH`]). A node that has moved also sends [`OP_SYNC`] to its
//!   previous home post office, which may be reachable only over LoRa.
//! * Hosts remove locally delivered mail; other copies expire. The legacy
//!   unauthenticated [`OP_DELIVERED`] opcode is retained for decoding.
//!
//! `Deposit` payload: `recipient (16) | op (1) | body`.
//!
//! | op | body |
//! |---|---|
//! | `STORE` (0) | the message, sealed to the recipient (static seal) |
//! | `SYNC` (1) | IDs the recipient already has (u32 BE each): deliver the rest to it |
//! | `FETCH` (2) | the asker's address (16), then IDs it already has: send the rest to the asker as `STORE` |
//! | `DELIVERED` (3) | Legacy IDs that have been delivered. Updated hosts ignore it. |
//! | `BUNDLE` (4) | Sealed messages for the recipient itself (n × length (1) \| sealed) |
//! | `RECEIPT` (5) | Recipient to original sender: IDs (u64 LE, [`receipt_id`]) of mail it got from a post office |
//!
//! A mail ID is the first 4 bytes of the message's packet ID
//! (`packet_id(Data, recipient, sealed)`), so every copy has the same ID.

use crate::Address;
use crate::dedup::packet_id;
use crate::packet::PacketType;

pub const OP_STORE: u8 = 0;
pub const OP_SYNC: u8 = 1;
pub const OP_FETCH: u8 = 2;
pub const OP_DELIVERED: u8 = 3;
/// Several sealed messages for the recipient in one packet, sent to the
/// recipient itself: body = n × (length (1) | sealed). One end-to-end ack
/// covers the bundle. Only to nodes that announce `appdata::CAP_BUNDLE`.
pub const OP_BUNDLE: u8 = 4;
/// Delivery receipt, sent by a recipient to the original sender after it read
/// mail that a post office delivered (a `BUNDLE`): body = n × [`receipt_id`]
/// (u64 LE). The `recipient` field of the `Deposit` is the sender.
pub const OP_RECEIPT: u8 = 5;

/// Copies a host keeps of every recipient's mail.
pub const REPLICAS: usize = 3;
/// How far a `MailSync` travels (hops).
pub const MAIL_SCOPE: u8 = 2;
/// A sender drops a message at its post office when it has not been
/// acknowledged within this time (ms).
pub const DEPOSIT_AFTER_MS: u64 = 45_000;
/// Post office classes (role flags bits 4-5).
pub const CLASS_NONE: u8 = 0;
pub const CLASS_SMALL: u8 = 1;
pub const CLASS_MEDIUM: u8 = 2;
pub const CLASS_LARGE: u8 = 3;

/// The 4-byte ID of a stored message.
pub fn mail_id(recipient: &Address, sealed: &[u8]) -> u32 {
    (packet_id(PacketType::Data, recipient, sealed) >> 32) as u32
}

/// The ID a receipt names: the packet ID of the sealed message as sent to
/// `recipient`. The sender can work it out from the copy it deposited, the
/// recipient from the message it received, so no ID travels inside the mail.
pub fn receipt_id(recipient: &Address, sealed: &[u8]) -> u64 {
    packet_id(PacketType::Data, recipient, sealed)
}

/// A decoded `Deposit` payload.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Deposit<'a> {
    pub recipient: Address,
    pub op: u8,
    pub body: &'a [u8],
}

impl<'a> Deposit<'a> {
    pub fn parse(p: &'a [u8]) -> Option<Deposit<'a>> {
        let recipient = Address::from_slice(p.get(..16)?)?;
        let op = *p.get(16)?;
        (op <= OP_RECEIPT).then_some(Deposit { recipient, op, body: &p[17..] })
    }

    /// The receipt IDs of a `RECEIPT` (see [`receipt_id`]).
    pub fn receipts(&self) -> impl Iterator<Item = u64> + '_ {
        let body = if self.op == OP_RECEIPT { self.body } else { &[] };
        body.chunks_exact(8).map(|c| u64::from_le_bytes([c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]]))
    }

    /// The IDs in a `SYNC` or `DELIVERED` body (after the asker's address
    /// in a `FETCH`).
    pub fn ids(&self) -> impl Iterator<Item = u32> + '_ {
        let skip = if self.op == OP_FETCH { 16 } else { 0 };
        self.body.get(skip..).unwrap_or(&[]).chunks_exact(4).map(|c| u32::from_be_bytes([c[0], c[1], c[2], c[3]]))
    }

    /// The sealed messages of a `BUNDLE`.
    pub fn bundle(&self) -> impl Iterator<Item = &'a [u8]> + 'a {
        let mut rest = if self.op == OP_BUNDLE { self.body } else { &[] };
        core::iter::from_fn(move || {
            let (&n, tail) = rest.split_first()?;
            let item = tail.get(..n as usize)?;
            rest = &tail[n as usize..];
            Some(item)
        })
    }

    /// The asker of a `FETCH`.
    pub fn asker(&self) -> Option<Address> {
        (self.op == OP_FETCH).then(|| self.body.get(..16).and_then(Address::from_slice)).flatten()
    }
}

/// Builds a `Deposit` payload into `out`; `false` if it does not fit.
pub fn build<const N: usize>(out: &mut heapless::Vec<u8, N>, recipient: &Address, op: u8, body: &[u8]) -> bool {
    out.clear();
    out.extend_from_slice(&recipient.0).is_ok() && out.push(op).is_ok() && out.extend_from_slice(body).is_ok()
}

/// Adds a sealed message to a bundle body; `false` if it does not fit in
/// `max` bytes (the whole Deposit payload is 17 + body).
pub fn bundle_push<const N: usize>(body: &mut heapless::Vec<u8, N>, sealed: &[u8], max: usize) -> bool {
    if sealed.len() > 255 || 17 + body.len() + 1 + sealed.len() > max {
        return false;
    }
    body.push(sealed.len() as u8).is_ok() && body.extend_from_slice(sealed).is_ok()
}

/// Builds an ID list body (at most `max` IDs).
pub fn id_body<const N: usize>(out: &mut heapless::Vec<u8, N>, ids: impl Iterator<Item = u32>, max: usize) {
    out.clear();
    for id in ids.take(max) {
        if out.extend_from_slice(&id.to_be_bytes()).is_err() {
            break;
        }
    }
}

/// Parses a `MailSync` payload: the asker's home post office and the IDs it
/// already has.
pub fn parse_sync(p: &[u8]) -> (Option<u16>, heapless::Vec<u32, 8>) {
    let home = p.get(..2).map(|b| u16::from_be_bytes([b[0], b[1]])).filter(|&h| h != 0xFFFF);
    let ids = p.get(2..p.len().saturating_sub(2)).unwrap_or(&[]);
    (home, ids.chunks_exact(4).take(8).map(|c| u32::from_be_bytes([c[0], c[1], c[2], c[3]])).collect())
}

/// Rendezvous (highest random weight) score of `office` for `recipient`:
/// every node computes the same ranking from the same list of offices, and
/// when an office joins or leaves only the recipients it ranks first for
/// move.
pub fn score(office: &Address, recipient: &Address) -> u64 {
    packet_id(PacketType::Deposit, office, &recipient.0)
}

/// The `k` offices (of `offices`) that keep `recipient`'s mail.
pub fn replicas<const K: usize>(recipient: &Address, offices: &[Address]) -> heapless::Vec<Address, K> {
    let mut best: heapless::Vec<(u64, Address), K> = heapless::Vec::new();
    for o in offices {
        let s = score(o, recipient);
        if best.iter().any(|(_, a)| a == o) {
            continue;
        }
        if best.is_full() {
            let (i, min) = best.iter().enumerate().min_by_key(|(_, (s, _))| *s).map(|(i, (s, _))| (i, *s)).unwrap_or((0, 0));
            if s <= min {
                continue;
            }
            best.remove(i);
        }
        let _ = best.push((s, *o));
    }
    crate::sort::sort_by(&mut best, |a, b| a.0 > b.0);
    best.into_iter().map(|(_, a)| a).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deposit_roundtrip_and_ids() {
        let r = Address([7; 16]);
        let mut v: heapless::Vec<u8, 64> = heapless::Vec::new();
        let mut body: heapless::Vec<u8, 32> = heapless::Vec::new();
        id_body(&mut body, [1u32, 2, 3].into_iter(), 8);
        assert!(build(&mut v, &r, OP_SYNC, &body));
        let d = Deposit::parse(&v).unwrap();
        assert_eq!((d.recipient, d.op), (r, OP_SYNC));
        assert_eq!(d.ids().collect::<Vec<_>>(), vec![1, 2, 3]);
        assert!(Deposit::parse(&[0u8; 16]).is_none(), "no op");
    }

    #[test]
    fn receipt_roundtrip() {
        let sender = Address([3; 16]);
        let me = Address([9; 16]);
        let id = receipt_id(&me, b"sealed message");
        let mut v: heapless::Vec<u8, 64> = heapless::Vec::new();
        assert!(build(&mut v, &sender, OP_RECEIPT, &id.to_le_bytes()));
        let d = Deposit::parse(&v).unwrap();
        assert_eq!((d.recipient, d.op), (sender, OP_RECEIPT));
        assert_eq!(d.receipts().collect::<Vec<_>>(), vec![id]);
        assert_eq!((id >> 32) as u32, mail_id(&me, b"sealed message"));
        assert_eq!(Deposit { recipient: sender, op: OP_STORE, body: &[0; 8] }.receipts().count(), 0);
    }

    #[test]
    fn replicas_are_stable_and_move_little() {
        let offices: Vec<Address> = (0..10u8).map(|i| Address([i; 16])).collect();
        let r = Address([0xAB; 16]);
        let a: heapless::Vec<Address, 3> = replicas(&r, &offices);
        assert_eq!(a.len(), 3);
        let b: heapless::Vec<Address, 3> = replicas(&r, &offices.iter().rev().copied().collect::<Vec<_>>());
        assert_eq!(a, b, "order of the list does not matter");
        // Removing an office that is not a replica changes nothing.
        let other = offices.iter().copied().find(|o| !a.contains(o)).unwrap();
        let fewer: Vec<Address> = offices.iter().copied().filter(|o| *o != other).collect();
        let c: heapless::Vec<Address, 3> = replicas(&r, &fewer);
        assert_eq!(a, c);
    }
}
