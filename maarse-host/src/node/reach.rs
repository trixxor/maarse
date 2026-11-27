//! How long what we know about where an address is stays true.
//!
//! A host publishes the addresses it has heard in the DHT and remembers which
//! of them are on its own radio. Neither may be kept for ever: a node goes
//! quiet, or moves to another gateway.

use super::*;

/// An address is published this long after its latest announce. Radio nodes
/// announce every 12 hours by default, ±10 %, so this survives one lost
/// announce.
const RADIO_LIFETIME: Duration = Duration::from_secs(30 * 3600);
/// The same for a host connected straight to us, which announces every 10
/// minutes.
const HOST_LIFETIME: Duration = Duration::from_secs(45 * 60);

/// An address we have heard, which we publish while it is alive.
pub(super) struct Heard {
    /// Its latest announce (payload without stamp).
    pub(super) announce: Vec<u8>,
    pub(super) hops: u8,
    /// When we last published it.
    pub(super) published: Option<Instant>,
    seen: Instant,
    lifetime: Duration,
}

impl Heard {
    fn new(announce: Vec<u8>, hops: u8, lifetime: Duration) -> Self {
        Self { announce, hops, published: None, seen: Instant::now(), lifetime }
    }

    pub(super) fn alive(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.seen) < self.lifetime
    }
}

impl Node {
    /// Our own announce: we can always be reached here.
    pub(super) fn note_self(&mut self, announce: Vec<u8>) {
        self.dht.heard.insert(self.addr, Heard::new(announce, 0, HOST_LIFETIME));
    }

    /// An announce heard on `link`. A copy of the announce we already have
    /// (a host answering a key request, say) does not show that the node is
    /// still there; a node without a clock sends no date to tell by.
    pub(super) fn note_heard(&mut self, link: LinkId, dest: Address, announce: &[u8], stamp: u32, hops: u8) {
        let previous = self.dht.heard.get(&dest);
        if stamp != 0 && previous.is_some_and(|h| maarse_crypto::announce_timestamp(&h.announce) == Some(stamp)) {
            return;
        }
        let published = previous.and_then(|h| h.published);
        // Only a host sends capabilities in its link greeting. A radio node on
        // Wi-Fi connects the same way, but announces as seldom as on the radio.
        let lifetime = if hops == 0 && self.link_caps.contains_key(&link) { HOST_LIFETIME } else { RADIO_LIFETIME };
        let mut heard = Heard::new(announce.to_vec(), hops, lifetime);
        heard.published = published;
        self.dht.heard.insert(dest, heard);
    }

    /// Another gateway publishes an address that we have as heard on our own
    /// radio, with an announce clearly newer than the last one we heard there:
    /// the node has moved, so we stop saying that it is here.
    pub(super) fn note_moved_elsewhere(&mut self, rec: &Record) {
        if rec.gw == self.addr || !(rec.verified_gateway() || self.dht.allowed_endpoints.contains(&rec.ep)) {
            return;
        }
        if !self.heard_via.get(&rec.dest).is_some_and(|(_, tcp, _)| !tcp) {
            return;
        }
        let ours = self.radio_heard.get(&rec.dest).copied();
        if !dht::clearly_newer(maarse_crypto::announce_timestamp(&rec.announce), ours) {
            return;
        }
        self.heard_via.remove(&rec.dest);
        self.router.forget_route(&rec.dest);
        if let Some(heard) = self.dht.heard.get_mut(&rec.dest) {
            heard.lifetime = Duration::ZERO;
        }
        self.log(format!("{} has moved: newer announce via {}", short(&rec.dest), short(&rec.gw)));
    }
}

#[cfg(test)]
mod tests {
    use super::super::file_interop_tests::cfg;
    use super::super::*;
    use std::fs;
    use std::sync::mpsc::channel;

    const GATEWAY_EP: &str = "198.51.100.2:4242";
    const MINUTE: Duration = Duration::from_secs(60);
    const HOUR: Duration = Duration::from_secs(3600);

    /// A public host, so it publishes what it hears.
    fn host(name: &str) -> (Node, PathBuf) {
        let dir = std::env::temp_dir().join(format!("maarse-reach-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let mut c = cfg(dir.clone());
        c.public_addr = Some("203.0.113.1:4242".into());
        (Node::new(&c).unwrap(), dir)
    }

    fn link(n: &mut Node, name: &str, is_host: bool) -> LinkId {
        let (tx, _rx) = channel();
        let id = n.links.len() as LinkId + 1;
        n.links.insert(id, (name.to_string(), tx.into()));
        if is_host {
            n.link_caps.insert(id, dht::Caps::default());
        }
        id
    }

    fn announce(who: &Identity, stamp: u32) -> Vec<u8> {
        who.announce(stamp, &[]).unwrap().to_vec()
    }

    /// An announce arrives on a link.
    fn hear(n: &mut Node, link: LinkId, who: &Identity, stamp: u32, hops: u8) {
        n.rx_link = Some(link);
        assert!(n.on_announce(who.address(), &announce(who, stamp), hops), "announce refused");
        n.rx_link = None;
    }

    fn publishes(n: &mut Node, who: &Identity, after: Duration) -> bool {
        n.dht_maintain_at(Instant::now() + after);
        n.dht.store.get(&who.address()).is_some_and(|r| r.gw == n.addr)
    }

    fn peer() -> Identity {
        Identity::generate(&mut OsRng)
    }

    #[test]
    fn a_radio_node_is_published_for_30_hours_after_its_last_announce() {
        for (after, expected) in [(29 * HOUR, true), (31 * HOUR, false)] {
            let (mut n, dir) = host("radio-life");
            let l = link(&mut n, "serial COM3", false);
            let who = peer();
            hear(&mut n, l, &who, unix_now(), 0);
            assert_eq!(publishes(&mut n, &who, after), expected, "after {after:?}");
            let _ = fs::remove_dir_all(dir);
        }
    }

    #[test]
    fn a_directly_connected_host_is_published_for_45_minutes() {
        for (after, expected) in [(44 * MINUTE, true), (46 * MINUTE, false)] {
            let (mut n, dir) = host("host-life");
            let l = link(&mut n, "tcp in 192.0.2.7:5000", true);
            let who = peer();
            hear(&mut n, l, &who, unix_now(), 0);
            assert_eq!(publishes(&mut n, &who, after), expected, "after {after:?}");
            let _ = fs::remove_dir_all(dir);
        }
    }

    #[test]
    fn only_a_direct_peer_that_says_it_is_a_host_gets_the_short_life() {
        // A radio node on Wi-Fi (no host greeting), and a node behind a host (one hop away):
        // both may announce only twice a day.
        for (is_host, hops) in [(false, 0), (true, 1)] {
            let (mut n, dir) = host("not-a-host");
            let l = link(&mut n, "tcp in 192.0.2.8:5000", is_host);
            let who = peer();
            hear(&mut n, l, &who, unix_now(), hops);
            assert!(publishes(&mut n, &who, HOUR), "host greeting {is_host}, hops {hops}");
            let _ = fs::remove_dir_all(dir);
        }
    }

    #[test]
    fn a_repeated_copy_of_an_announce_does_not_count_as_hearing_it_again() {
        let (mut n, dir) = host("replay");
        let direct = link(&mut n, "tcp in 192.0.2.9:5000", true);
        let radio = link(&mut n, "serial COM3", false);
        let who = peer();
        let stamp = unix_now();
        hear(&mut n, direct, &who, stamp, 0);
        // The same announce again, say as a host's answer to a key request.
        hear(&mut n, radio, &who, stamp, 0);
        assert!(!publishes(&mut n, &who, HOUR));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn every_announce_of_a_node_without_a_clock_counts() {
        let (mut n, dir) = host("clockless");
        let direct = link(&mut n, "tcp in 192.0.2.9:5000", true);
        let radio = link(&mut n, "serial COM3", false);
        let who = peer();
        hear(&mut n, direct, &who, 0, 0);
        hear(&mut n, radio, &who, 0, 0);
        assert!(publishes(&mut n, &who, HOUR));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn what_a_lookup_found_is_not_published_as_our_own() {
        let (mut n, dir) = host("lookup-result");
        let (who, gateway) = (peer(), peer());
        let record = Record::signed(&gateway, who.address(), GATEWAY_EP.into(), 0, announce(&who, unix_now()));
        n.dht_done(who.address(), false, dht::Lookup { record: Some(record), ..Default::default() });
        assert!(!publishes(&mut n, &who, Duration::ZERO));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn an_address_is_looked_up_again_once_it_has_gone_quiet() {
        let (mut n, dir) = host("lookup-again");
        let l = link(&mut n, "serial COM3", false);
        let who = peer();
        hear(&mut n, l, &who, unix_now(), 0);
        n.dht_maybe_lookup_at(who.address(), Instant::now() + HOUR);
        assert_eq!(n.dht.lookups, 0, "just heard: no lookup");
        // Published, so our own store holds a record; it must not answer the question.
        assert!(publishes(&mut n, &who, Duration::ZERO));
        n.dht_maybe_lookup_at(who.address(), Instant::now() + 31 * HOUR);
        assert_eq!(n.dht.lookups, 1);
        assert_eq!(n.dht.found, 0, "our own old record is not an answer");
        let _ = fs::remove_dir_all(dir);
    }

    /// We heard a node on our radio two hours ago; now another gateway publishes it with an
    /// announce dated `newer` seconds after the one we heard.
    fn heard_on_radio_then_published_elsewhere(name: &str, newer: u32) -> (Node, Identity, Identity, Record, PathBuf) {
        let (mut n, dir) = host(name);
        let l = link(&mut n, "serial COM3", false);
        let (who, gateway) = (peer(), peer());
        let then = unix_now() - 7200;
        hear(&mut n, l, &who, then, 0);
        n.router.add_route(who.address(), Default::default(), n.now_ms());
        let record = Record::signed(&gateway, who.address(), GATEWAY_EP.into(), 1, announce(&who, then + newer));
        (n, who, gateway, record, dir)
    }

    fn store_request(n: &mut Node, gateway: &Identity, record: Record) {
        let request = dht::encode_req(&gateway.address(), Some(GATEWAY_EP), &dht::Req::Store(record));
        n.dht_serve(&request);
    }

    fn on_radio(n: &Node, who: &Identity) -> bool {
        n.heard_via.get(&who.address()).is_some_and(|(_, tcp, _)| !tcp)
    }

    #[test]
    fn a_much_newer_announce_elsewhere_ends_heard_on_our_radio() {
        let (mut n, who, gateway, record, dir) = heard_on_radio_then_published_elsewhere("moved", 3700);
        store_request(&mut n, &gateway, record);
        assert!(!on_radio(&n, &who));
        assert!(!n.router.has_route(&who.address()), "the radio route is stale too");
        assert!(!publishes(&mut n, &who, Duration::ZERO), "we stop publishing it");
        assert_eq!(n.dht.store.get(&who.address()).map(|r| r.gw), Some(gateway.address()));
        n.dht_maybe_lookup_at(who.address(), Instant::now());
        assert_eq!(n.dht.found, 1);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_lookup_that_finds_a_much_newer_announce_reaches_the_node_over_tcp() {
        let (mut n, who, _gateway, record, dir) = heard_on_radio_then_published_elsewhere("moved-lookup", 3700);
        n.dht_done(who.address(), false, dht::Lookup { record: Some(record), ..Default::default() });
        assert!(!on_radio(&n, &who));
        assert!(n.path_kind(&who.address()) == PathKind::Tcp);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_slightly_newer_announce_elsewhere_keeps_the_radio() {
        // Missing one announce on the radio is no proof that the node has gone.
        let (mut n, who, gateway, record, dir) = heard_on_radio_then_published_elsewhere("lost-one", 600);
        store_request(&mut n, &gateway, record);
        assert!(on_radio(&n, &who));
        assert!(n.router.has_route(&who.address()));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn an_unproven_gateway_cannot_take_a_node_off_the_radio() {
        let (mut n, who, gateway, record, dir) = heard_on_radio_then_published_elsewhere("unsigned", 3700);
        // The older record format: the same bytes without the 136-byte gateway proof.
        let mut request = dht::encode_req(&gateway.address(), Some(GATEWAY_EP), &dht::Req::Store(record));
        request.truncate(request.len() - 136);
        assert!(matches!(dht::decode_req(&request), Some((_, _, dht::Req::Store(_)))));
        n.dht_serve(&request);
        assert!(on_radio(&n, &who));
        let _ = fs::remove_dir_all(dir);
    }
}
