//! Pieces of the Wi-Fi hotspot (ESP32 nodes): a tiny DHCP server so a phone gets an address, a fair share of
//! the radio's duty cycle per phone, and the rule that phones are clients only. All of it is plain code without
//! hardware, so it is tested here; the firmware (`firmware/esp/src/hotspot.rs`) only moves bytes.

use crate::airtime::LoraParams;
use crate::packet::Header;

/// Port the phones connect to (the backbone link framing, `length (u16 LE) | packet`).
pub const PORT: u16 = 4244;
/// The node's own address on the hotspot network (24-bit mask).
pub const SERVER_IP: [u8; 4] = [192, 168, 4, 1];
/// Most phones at once.
pub const MAX_CLIENTS: usize = 4;
/// The first address handed out; slot `i` gets `FIRST_HOST + i`.
pub const FIRST_HOST: u8 = 10;
const LEASE_SECS: u32 = 3600;

// ---------------------------------------------------------------------------
// DHCP

/// Which MAC holds which address. An address belongs to a MAC for as long as the node runs (a phone that comes
/// back gets the same one); when all slots are taken, the oldest is reused.
#[derive(Debug, Default)]
pub struct Leases {
    macs: [Option<[u8; 6]>; MAX_CLIENTS * 2],
    next: usize,
}

impl Leases {
    pub const SLOTS: usize = MAX_CLIENTS * 2;

    pub fn new() -> Self {
        Self::default()
    }

    /// The address for `mac`, handing out a new one if it has none.
    pub fn ip_for(&mut self, mac: &[u8; 6]) -> [u8; 4] {
        let i = match self.macs.iter().position(|m| m.as_ref() == Some(mac)) {
            Some(i) => i,
            None => {
                let i = self.macs.iter().position(Option::is_none).unwrap_or_else(|| {
                    let i = self.next % Self::SLOTS;
                    self.next += 1;
                    i
                });
                self.macs[i] = Some(*mac);
                i
            }
        };
        [SERVER_IP[0], SERVER_IP[1], SERVER_IP[2], FIRST_HOST + i as u8]
    }
}

const MAGIC: [u8; 4] = [99, 130, 83, 99];
const DISCOVER: u8 = 1;
const OFFER: u8 = 2;
const REQUEST: u8 = 3;
const ACK: u8 = 5;
const NAK: u8 = 6;

/// The answer to a DHCP request (`DISCOVER` gets an offer, `REQUEST` an acknowledgement, or a refusal if the
/// phone asks for an address that is not its own), written to `out`. Returns its length, or `None` for anything
/// that is not a request we answer. The offer has no router and no DNS server: the hotspot has no internet, and
/// the phone should keep using its mobile data for that.
pub fn dhcp_reply(req: &[u8], leases: &mut Leases, out: &mut [u8; 300]) -> Option<usize> {
    if req.len() < 241 || req[0] != 1 || req[1] != 1 || req[2] != 6 || req[236..240] != MAGIC {
        return None;
    }
    let mut kind = 0u8;
    let mut wanted: Option<[u8; 4]> = None;
    let mut i = 240;
    while i < req.len() {
        match req[i] {
            0 => i += 1,
            255 => break,
            tag => {
                let len = *req.get(i + 1)? as usize;
                let body = req.get(i + 2..i + 2 + len)?;
                match (tag, body) {
                    (53, [t]) => kind = *t,
                    (50, b) if b.len() == 4 => wanted = Some([b[0], b[1], b[2], b[3]]),
                    _ => {}
                }
                i += 2 + len;
            }
        }
    }
    let mac: [u8; 6] = req[28..34].try_into().ok()?;
    let mine = leases.ip_for(&mac);
    let (reply, yiaddr) = match kind {
        DISCOVER => (OFFER, mine),
        REQUEST => {
            // The address asked for is in "requested IP" (renewing clients use ciaddr instead).
            let asked = wanted.unwrap_or([req[12], req[13], req[14], req[15]]);
            if asked == mine { (ACK, mine) } else { (NAK, [0; 4]) }
        }
        _ => return None,
    };
    out.fill(0);
    out[0] = 2;
    out[1] = 1;
    out[2] = 6;
    out[4..8].copy_from_slice(&req[4..8]); // xid
    out[10..12].copy_from_slice(&req[10..12]); // flags (broadcast bit)
    out[16..20].copy_from_slice(&yiaddr);
    if reply != NAK {
        out[20..24].copy_from_slice(&SERVER_IP);
    }
    out[28..44].copy_from_slice(&req[28..44]); // chaddr
    out[236..240].copy_from_slice(&MAGIC);
    let mut o = 240;
    let mut put = |tag: u8, body: &[u8]| {
        out[o] = tag;
        out[o + 1] = body.len() as u8;
        out[o + 2..o + 2 + body.len()].copy_from_slice(body);
        o += 2 + body.len();
    };
    put(53, &[reply]);
    put(54, &SERVER_IP);
    if reply != NAK {
        put(51, &LEASE_SECS.to_be_bytes());
        put(1, &[255, 255, 255, 0]);
    }
    out[o] = 255;
    Some(o + 1)
}

// ---------------------------------------------------------------------------
// A fair share of the radio

/// Time on air of a packet in milliseconds with the node's radio settings
/// (used to share the radio fairly; the radio itself counts the real airtime).
pub fn estimate_air_ms(len: usize, radio: &LoraParams) -> u32 {
    radio.airtime_ms(len)
}

/// The most a phone may save up, in milliseconds of airtime (about three full
/// packets at SF9). A single longer packet may be saved up for.
pub const BURST_MS: u32 = 4_000;

/// Airtime a phone may use. It fills at the phone's share of the node's duty
/// cycle (all connected phones split it equally) up to [`BURST_MS`]; a packet
/// is sent when the bucket holds its airtime.
#[derive(Clone, Copy, Debug)]
pub struct Bucket {
    /// Milliseconds of airtime available, times 1000 (so a share of a few permille is not lost to rounding).
    micro: u64,
    last_ms: u64,
}

impl Bucket {
    pub fn new(now_ms: u64) -> Self {
        Bucket { micro: BURST_MS as u64 * 1000, last_ms: now_ms }
    }

    /// Permille of the time one phone gets.
    fn share(clients: u32, duty_permille: u16) -> u64 {
        (duty_permille as u64 / clients.max(1) as u64).max(1)
    }

    fn refill(&mut self, now_ms: u64, clients: u32, duty_permille: u16, cost_ms: u32) {
        let dt = now_ms.saturating_sub(self.last_ms);
        self.last_ms = now_ms;
        let most = BURST_MS.max(cost_ms) as u64 * 1000;
        self.micro = self.micro.saturating_add(dt.saturating_mul(Self::share(clients, duty_permille))).min(most);
    }

    /// Takes `cost_ms` of airtime if the bucket holds it.
    pub fn take(&mut self, now_ms: u64, clients: u32, duty_permille: u16, cost_ms: u32) -> bool {
        self.refill(now_ms, clients, duty_permille, cost_ms);
        let need = cost_ms as u64 * 1000;
        if self.micro >= need {
            self.micro -= need;
            true
        } else {
            false
        }
    }

    /// How long until `cost_ms` is available.
    pub fn wait_ms(&mut self, now_ms: u64, clients: u32, duty_permille: u16, cost_ms: u32) -> u32 {
        self.refill(now_ms, clients, duty_permille, cost_ms);
        let need = (cost_ms as u64 * 1000).saturating_sub(self.micro);
        need.div_ceil(Self::share(clients, duty_permille)).min(u32::MAX as u64) as u32
    }
}

/// One bucket per address on the hotspot, so a phone that connects again
/// goes on where it was instead of starting with a full bucket.
#[derive(Debug, Default)]
pub struct Shares {
    buckets: [Option<Bucket>; Leases::SLOTS],
}

impl Shares {
    pub fn new() -> Self {
        Self::default()
    }

    /// The bucket of the address ending in `host`.
    fn bucket(&mut self, host: u8, now_ms: u64) -> &mut Bucket {
        let i = host.wrapping_sub(FIRST_HOST) as usize % Leases::SLOTS;
        self.buckets[i].get_or_insert_with(|| Bucket::new(now_ms))
    }

    pub fn take(&mut self, host: u8, now_ms: u64, clients: u32, duty_permille: u16, cost_ms: u32) -> bool {
        self.bucket(host, now_ms).take(now_ms, clients, duty_permille, cost_ms)
    }

    pub fn wait_ms(&mut self, host: u8, now_ms: u64, clients: u32, duty_permille: u16, cost_ms: u32) -> u32 {
        self.bucket(host, now_ms).wait_ms(now_ms, clients, duty_permille, cost_ms)
    }
}

// ---------------------------------------------------------------------------
// Phones are clients

/// A phone that joins a public repeater is a client: it sends its own packets and nothing it has passed on for
/// others. A packet that has already been forwarded (`hops > 0`) or cannot be read is not accepted from a phone.
pub fn from_client_ok(packet: &[u8]) -> bool {
    matches!(Header::decode(packet), Ok((h, _)) if h.hops == 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packet::{PacketType, encode};
    use crate::Address;

    fn request(kind: u8, mac: [u8; 6], wanted: Option<[u8; 4]>) -> Vec<u8> {
        let mut p = vec![0u8; 240];
        p[0] = 1;
        p[1] = 1;
        p[2] = 6;
        p[4..8].copy_from_slice(&[1, 2, 3, 4]);
        p[10] = 0x80;
        p[28..34].copy_from_slice(&mac);
        p[236..240].copy_from_slice(&MAGIC);
        p.extend_from_slice(&[53, 1, kind]);
        if let Some(w) = wanted {
            p.extend_from_slice(&[50, 4, w[0], w[1], w[2], w[3]]);
        }
        p.push(255);
        p
    }

    fn option(reply: &[u8], tag: u8) -> Option<Vec<u8>> {
        let mut i = 240;
        while i < reply.len() && reply[i] != 255 {
            let len = reply[i + 1] as usize;
            if reply[i] == tag {
                return Some(reply[i + 2..i + 2 + len].to_vec());
            }
            i += 2 + len;
        }
        None
    }

    #[test]
    fn dhcp_gives_each_phone_its_own_address_and_keeps_it() {
        let mut leases = Leases::new();
        let mut out = [0u8; 300];
        let a = [2, 0, 0, 0, 0, 1];
        let b = [2, 0, 0, 0, 0, 2];
        let n = dhcp_reply(&request(DISCOVER, a, None), &mut leases, &mut out).unwrap();
        let offer_a = out[16..20].to_vec();
        assert_eq!(option(&out[..n], 53), Some(vec![OFFER]));
        assert_eq!(&out[4..8], &[1, 2, 3, 4], "same transaction id");
        assert_eq!(option(&out[..n], 54), Some(SERVER_IP.to_vec()));
        assert_eq!(option(&out[..n], 3), None, "no router: the hotspot has no internet");
        assert_eq!(option(&out[..n], 6), None, "no DNS server");
        // The request for that address is acknowledged.
        let w = [offer_a[0], offer_a[1], offer_a[2], offer_a[3]];
        let n = dhcp_reply(&request(REQUEST, a, Some(w)), &mut leases, &mut out).unwrap();
        assert_eq!(option(&out[..n], 53), Some(vec![ACK]));
        assert_eq!(&out[16..20], &offer_a[..]);
        // The other phone gets another address; the first one gets the same again.
        dhcp_reply(&request(DISCOVER, b, None), &mut leases, &mut out).unwrap();
        assert_ne!(&out[16..20], &offer_a[..]);
        dhcp_reply(&request(DISCOVER, a, None), &mut leases, &mut out).unwrap();
        assert_eq!(&out[16..20], &offer_a[..]);
        // An address that is not the phone's own is refused.
        let n = dhcp_reply(&request(REQUEST, a, Some([192, 168, 4, 99])), &mut leases, &mut out).unwrap();
        assert_eq!(option(&out[..n], 53), Some(vec![NAK]));
    }

    #[test]
    fn dhcp_ignores_what_it_does_not_understand() {
        let mut leases = Leases::new();
        let mut out = [0u8; 300];
        assert!(dhcp_reply(&[0; 10], &mut leases, &mut out).is_none());
        let mut bad = request(DISCOVER, [2; 6], None);
        bad[236] = 0;
        assert!(dhcp_reply(&bad, &mut leases, &mut out).is_none());
        assert!(dhcp_reply(&request(8, [2; 6], None), &mut leases, &mut out).is_none(), "INFORM is not answered");
        // An option that runs past the end of the packet.
        let mut cut = request(DISCOVER, [2; 6], None);
        cut.truncate(cut.len() - 2);
        cut.extend_from_slice(&[53, 9]);
        assert!(dhcp_reply(&cut, &mut leases, &mut out).is_none());
    }

    #[test]
    fn leases_reuse_the_oldest_slot_when_full() {
        let mut l = Leases::new();
        for i in 0..Leases::SLOTS as u8 {
            l.ip_for(&[i, 0, 0, 0, 0, 0]);
        }
        let first = l.ip_for(&[0, 0, 0, 0, 0, 0]);
        assert_eq!(first[3], FIRST_HOST);
        let extra = l.ip_for(&[99, 0, 0, 0, 0, 0]);
        assert_eq!(extra[3], FIRST_HOST, "the oldest slot is reused");
    }

    #[test]
    fn reconnecting_does_not_give_a_phone_a_new_burst() {
        let mut s = Shares::new();
        let (phone, other) = (FIRST_HOST, FIRST_HOST + 1);
        assert!(s.take(phone, 0, 1, 100, 4000), "the first burst");
        // The same address again, as after a reconnect.
        assert!(!s.take(phone, 100, 1, 100, 1000));
        assert!(s.take(other, 100, 2, 100, 1000), "another phone has its own share");
        let later = 100 + s.wait_ms(phone, 100, 1, 100, 1000) as u64;
        assert!(s.take(phone, later, 1, 100, 1000));
    }

    #[test]
    fn a_packet_longer_than_the_burst_is_sent_in_the_end() {
        let slow = crate::airtime::LoraParams { sf: 12, bw_hz: 125_000, cr: 1, preamble: 16 };
        let cost = estimate_air_ms(255, &slow);
        assert!(cost > BURST_MS);
        let mut b = Bucket::new(0);
        let wait = b.wait_ms(0, 1, 100, cost);
        assert!(b.take(wait as u64, 1, 100, cost), "after {wait} ms");
    }

    #[test]
    fn a_phone_gets_its_share_of_the_duty_cycle() {
        // One phone: 10 % of the time. After two 1.8 s packets of the 4 s burst, the next one needs 1.4 s more
        // airtime, which takes 14 s.
        let mut b = Bucket::new(0);
        assert!(b.take(0, 1, 100, 1800) && b.take(0, 1, 100, 1800));
        assert!(!b.take(0, 1, 100, 1800), "the burst is used up");
        let wait = b.wait_ms(0, 1, 100, 1800);
        assert!((13_000..=15_000).contains(&wait), "wait {wait}");
        assert!(b.take(wait as u64, 1, 100, 1800));
        // Four phones split it: the same packet waits four times as long.
        let mut b = Bucket::new(0);
        assert!(b.take(0, 4, 100, 1800) && b.take(0, 4, 100, 1800));
        let wait4 = b.wait_ms(0, 4, 100, 1800);
        assert!(wait4 > 3 * wait, "wait {wait4} against {wait}");
    }

    #[test]
    fn over_an_hour_phones_never_use_more_than_the_duty_cycle() {
        // Four phones that always have something to send: together they get about 10 % of 3600 s.
        let mut buckets: Vec<Bucket> = (0..4).map(|_| Bucket::new(0)).collect();
        let mut used = 0u64;
        for t in (0..3_600_000u64).step_by(100) {
            for b in buckets.iter_mut() {
                if b.take(t, 4, 100, 800) {
                    used += 800;
                }
            }
        }
        // The starting bursts add 4 phones x 4 s.
        assert!(used <= 360_000 + 16_000 + 4_000, "used {used} ms");
        assert!(used >= 340_000, "used {used} ms: the duty cycle should be used");
    }

    #[test]
    fn only_a_phones_own_packets_are_accepted() {
        let own = encode(&Header::new(PacketType::Announce, Address([1; 16])), &[1, 2, 3]).unwrap();
        assert!(from_client_ok(&own));
        let mut forwarded = Header::new(PacketType::Announce, Address([1; 16]));
        forwarded.hops = 1;
        let fwd = encode(&forwarded, &[1, 2, 3]).unwrap();
        assert!(!from_client_ok(&fwd), "a forwarded packet is not a phone's own");
        assert!(!from_client_ok(&[1, 2]), "nor is rubbish");
    }
}
