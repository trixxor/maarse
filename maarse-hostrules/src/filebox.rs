//! Files and routes: how large a file may be along the current route.
//!
//! The host sends packets out on all its links (TCP backbone and an optional
//! serial LoRa modem). Which way a recipient is reached, it therefore only
//! knows from where it last heard it: the link its announce came in on, and
//! how many hops away it was. [`classify`] says:
//!
//! - [`PathKind::Tcp`]: the recipient was heard over a TCP link, and both it and
//!   all intermediate hops on the known route are hosts we know are on the backbone
//!   (direct TCP neighbours, heard with 0 hops, or contacts in the DHT). A
//!   web client at a host has the host's address and therefore counts too.
//! - [`PathKind::Lora`]: the recipient was heard over serial/LoRa, or it was
//!   heard over TCP, but through a hop we do not know is a host (e.g. a
//!   radio node behind another host's modem).
//! - [`PathKind::Unknown`]: no knowledge (never heard, or the link it was
//!   heard on has gone down).
//!
//! Assumption: two hosts that are both on the backbone are connected over TCP.
//! When in doubt, the answer is never `Tcp`, and then the 8 KB limit applies.
//!
//! # Policy
//!
//! - `Tcp`: up to `--max-file` (default 1 MB).
//! - `Lora`: at most [`LORA_MAX_FILE`] (8 KB, same as the firmware).
//! - `Unknown`: the recipient cannot be reached now. With the file mailbox enabled the
//!   file may be up to 1 MB: it is put in the mailbox and delivered when the
//!   recipient shows up again. Files over 8 KB are never sent over a route
//!   that is not `Tcp` (the chunks are held back), so they wait in the mailbox
//!   for a TCP route or expire after [`FILEBOX_TTL_MS`].

use maarse_core::airtime::LoraParams;
use maarse_core::companion::RadioConfig;
use maarse_crypto::seal::CHUNK_DATA;
use std::collections::HashSet;

/// Largest file over LoRa (same as the firmware's file buffer).
pub const LORA_MAX_FILE: u32 = maarse_crypto::files::MAX_FILE as u32;
/// Files in the mailbox expire after the same time as messages (7 days, ms).
pub const FILEBOX_TTL_MS: u64 = 7 * 24 * 3600 * 1000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathKind {
    Tcp,
    Lora,
    Unknown,
}

/// What the host knows about the way to a recipient.
pub struct PathFacts<'a> {
    /// The link the recipient's announce last came in on: `Some(true)` for TCP,
    /// `Some(false)` for serial/LoRa, `None` if unknown.
    pub heard_tcp: Option<bool>,
    /// Intermediate hops (path ids) on the known route, if the router has one.
    pub route: Option<&'a [u16]>,
    /// Is the recipient itself a known backbone host?
    pub dest_is_host: bool,
    /// Path ids of known backbone hosts.
    pub host_ids: &'a HashSet<u16>,
}

pub fn classify(f: &PathFacts) -> PathKind {
    match f.heard_tcp {
        None => PathKind::Unknown,
        Some(false) => PathKind::Lora,
        Some(true) => {
            let hops_ok = f.route.is_none_or(|p| p.iter().all(|id| f.host_ids.contains(id)));
            if f.dest_is_host && hops_ok {
                PathKind::Tcp
            } else {
                PathKind::Lora
            }
        }
    }
}

/// Limit for a file along a route of type `kind`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limit {
    pub max_bytes: u32,
    pub via_lora: bool,
    pub mailbox: bool,
    pub est_airtime_s: u32,
}

pub fn limit(kind: PathKind, max_file: u32, filebox_on: bool, radio: &RadioConfig) -> Limit {
    let lora = |mailbox| {
        let max = LORA_MAX_FILE.min(max_file);
        Limit { max_bytes: max, via_lora: true, mailbox, est_airtime_s: est_airtime_s(max, radio) }
    };
    match kind {
        PathKind::Tcp => Limit { max_bytes: max_file, via_lora: false, mailbox: false, est_airtime_s: 0 },
        PathKind::Lora => lora(false),
        PathKind::Unknown if filebox_on => Limit {
            max_bytes: max_file,
            via_lora: true,
            mailbox: true,
            est_airtime_s: est_airtime_s(LORA_MAX_FILE.min(max_file), radio),
        },
        PathKind::Unknown => lora(false),
    }
}

/// Estimated time to send `size` bytes as file chunks over LoRa with
/// the radio settings `radio`. Each chunk counts as a full packet of 255
/// bytes (plus one for the offer), and each chunk is acknowledged with a short packet
/// (about 40 bytes), which also occupies the channel. Duty cycle is measured over an hour
/// (see `maarse_core::duty`): if the sender's airtime fits within the hour's
/// budget, it is the airtime; otherwise it is airtime / duty cycle.
pub fn est_airtime_s(size: u32, radio: &RadioConfig) -> u32 {
    let p = LoraParams {
        sf: radio.sf,
        bw_hz: radio.bw_hz,
        cr: radio.cr.saturating_sub(4).max(1),
        preamble: radio.preamble,
    };
    let packets = (size as usize).div_ceil(CHUNK_DATA) as u64 + 1;
    let tx_ms = packets * p.airtime_ms(maarse_core::MAX_PACKET) as u64;
    let ack_ms = packets * p.airtime_ms(40) as u64;
    let duty = radio.duty_permille.clamp(1, 1000) as u64;
    let budget_ms = maarse_core::duty::WINDOW_MS * duty / 1000;
    let ms = if tx_ms <= budget_ms { tx_ms + ack_ms } else { tx_ms * 1000 / duty };
    ms.div_ceil(1000) as u32
}

/// Has a file stored at `stored_s` (unix seconds) expired at `now_s`?
pub fn filebox_expired(stored_s: u64, now_s: u64) -> bool {
    now_s.saturating_sub(stored_s) >= FILEBOX_TTL_MS / 1000
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(v: &[u16]) -> HashSet<u16> {
        v.iter().copied().collect()
    }

    #[test]
    fn classify_routes() {
        let hosts = ids(&[0x1111, 0x2222]);
        let f = |heard_tcp, route: Option<&'static [u16]>, dest_is_host| {
            classify(&PathFacts { heard_tcp, route, dest_is_host, host_ids: &hosts })
        };
        // Direct TCP neighbour (or a web client at it).
        assert_eq!(f(Some(true), None, true), PathKind::Tcp);
        assert_eq!(f(Some(true), Some(&[]), true), PathKind::Tcp);
        // Via known hosts on the backbone.
        assert_eq!(f(Some(true), Some(&[0x1111, 0x2222]), true), PathKind::Tcp);
        // An unknown intermediate hop (e.g. a LoRa repeater behind a host).
        assert_eq!(f(Some(true), Some(&[0x1111, 0x3333]), true), PathKind::Lora);
        // Radio node behind another host's modem.
        assert_eq!(f(Some(true), None, false), PathKind::Lora);
        // Heard over the serial modem.
        assert_eq!(f(Some(false), None, true), PathKind::Lora);
        assert_eq!(f(Some(false), Some(&[]), false), PathKind::Lora);
        // No knowledge.
        assert_eq!(f(None, None, true), PathKind::Unknown);
        assert_eq!(f(None, Some(&[]), true), PathKind::Unknown);
    }

    #[test]
    fn limits_by_path() {
        let r = RadioConfig::EU_DEFAULT;
        let mb = 1024 * 1024;
        let t = limit(PathKind::Tcp, mb, true, &r);
        assert_eq!((t.max_bytes, t.via_lora, t.mailbox, t.est_airtime_s), (mb, false, false, 0));
        let l = limit(PathKind::Lora, mb, true, &r);
        assert_eq!((l.max_bytes, l.via_lora, l.mailbox), (8192, true, false));
        assert!(l.est_airtime_s > 30 && l.est_airtime_s < 300, "{}", l.est_airtime_s);
        let u = limit(PathKind::Unknown, mb, true, &r);
        assert_eq!((u.max_bytes, u.via_lora, u.mailbox), (mb, true, true));
        let u = limit(PathKind::Unknown, mb, false, &r);
        assert_eq!((u.max_bytes, u.mailbox), (8192, false));
        // A small --max-file beats 8 KB.
        assert_eq!(limit(PathKind::Lora, 4096, true, &r).max_bytes, 4096);
    }

    #[test]
    fn airtime_grows_with_size_and_sf() {
        let r = RadioConfig::EU_DEFAULT;
        assert!(est_airtime_s(8192, &r) > est_airtime_s(1000, &r));
        let sf12 = RadioConfig { sf: 12, ..r };
        assert!(est_airtime_s(8192, &sf12) > est_airtime_s(8192, &r));
        // 1 MB over LoRa takes hours.
        assert!(est_airtime_s(1 << 20, &r) > 3600 * 3);
    }

    #[test]
    fn mailbox_ttl_is_seven_days() {
        assert_eq!(FILEBOX_TTL_MS, 604_800_000);
        let t = 1_800_000_000;
        assert!(!filebox_expired(t, t + 7 * 24 * 3600 - 1));
        assert!(filebox_expired(t, t + 7 * 24 * 3600));
        assert!(!filebox_expired(t + 10, t), "a time in the future is not expired");
    }
}
