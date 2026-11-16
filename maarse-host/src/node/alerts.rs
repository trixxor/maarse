//! Alerts v2 on the host: verify, drop replays, show as a separate event,
//! and issue alerts with a separate issuer key (see maarse_crypto::alert and
//! docs/maybe/alerting-review.html).

use super::*;
use maarse_core::companion::{AlertMeta, alert_trust};
use maarse_crypto::alert::{self, Alert, Area, Cert, Check, ReplayGuard, Via};

/// Repeats of the issuer's certificate: with the first alert, then at most
/// once an hour (receivers cache it).
const CERT_EVERY: Duration = Duration::from_secs(3600);

pub(super) struct Alerts {
    /// Root keys whose issuer certificates we accept (`--alert-root`).
    roots: Vec<PublicIdentity>,
    /// Verified certificates, newest per issuer.
    certs: Vec<Cert>,
    guard: ReplayGuard<256>,
    /// Our issuer key (`--issuer-key`), kept apart from the node identity.
    issuer: Option<Identity>,
    /// Our certificate (`--issuer-cert`), sent before our alerts.
    my_cert: Option<Vec<u8>>,
    cert_sent: Option<Instant>,
    last_serial: u32,
    dir: PathBuf,
}

impl Alerts {
    pub(super) fn load(cfg: &crate::Config) -> std::io::Result<Self> {
        let dir = cfg.data_dir.clone();
        let issuer = match &cfg.issuer_key {
            Some(p) => {
                let b = fs::read(p)?;
                let k: [u8; 64] = b.as_slice().try_into().map_err(|_| std::io::Error::other("--issuer-key: expected a 64-byte key file"))?;
                Some(Identity::from_secret_bytes(&k))
            }
            None => None,
        };
        let my_cert = match &cfg.issuer_cert {
            Some(p) => Some(fs::read(p)?),
            None => None,
        };
        let guard = fs::read(dir.join("alerts.bin")).map(|b| ReplayGuard::from_bytes(&b)).unwrap_or_default();
        let last_serial = fs::read_to_string(dir.join("alert_serial.txt")).ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0);
        let mut a = Alerts { roots: cfg.alert_roots.clone(), certs: Vec::new(), guard, issuer, my_cert, cert_sent: None, last_serial, dir };
        if let Some(c) = a.my_cert.clone() {
            a.note_cert(&c);
        }
        Ok(a)
    }

    /// Does this host take part in alerts (and so join the bulletin channel)?
    pub(super) fn active(&self) -> bool {
        !self.roots.is_empty() || self.issuer.is_some()
    }

    pub(super) fn issuer_public(&self) -> Option<PublicIdentity> {
        self.issuer.as_ref().map(|i| i.public())
    }

    /// Caches a certificate if a root signed it.
    fn note_cert(&mut self, plain: &[u8]) -> Option<String> {
        let c = alert::verify_cert(plain, &self.roots)?;
        let name = c.name.to_string();
        self.certs.retain(|o| o.issuer != c.issuer || o.not_after > c.not_after);
        if !self.certs.iter().any(|o| o.issuer == c.issuer) {
            self.certs.push(c);
        }
        Some(name)
    }

    fn save_guard(&self) {
        let mut b = vec![0u8; 4 + 20 * 256];
        let n = self.guard.to_bytes(&mut b);
        let _ = fs::write(self.dir.join("alerts.bin"), &b[..n]);
    }
}

fn meta_of(a: &Alert) -> AlertMeta {
    AlertMeta {
        status: a.status,
        kind: a.kind,
        event: a.event,
        severity: a.severity,
        expires_min: a.expires_min,
        reference: a.reference,
        area: a.area.map(|r| (r.lat_e5, r.lon_e5, r.radius_100m)),
    }
}

fn clock() -> Option<u32> {
    // A host without a set clock (before 2023) is treated as clockless.
    Some(unix_now()).filter(|&t| t > 1_672_531_200)
}

impl Node {
    /// A message on the bulletin channel: a certificate, an alert v2 or an
    /// old v1 bulletin. Anything unverified is dropped and never shown.
    pub(super) fn on_bulletin(&mut self, from: Address, hops: u8, plain: &[u8]) {
        if alert::is_cert(plain) {
            match self.alerts.note_cert(plain) {
                Some(name) => self.log(format!("alert certificate for \"{name}\" ({})", short(&from))),
                None => self.log(format!("dropped an alert certificate not signed by a root key ({})", short(&from))),
            }
            return;
        }
        let (a, trust, name) = if alert::is_alert(plain) {
            match alert::verify(&from, plain, &self.bulletin_keys, &self.alerts.certs) {
                Ok((a, Via::Key)) => (a, alert_trust::KEY, String::new()),
                Ok((a, Via::Cert(n))) => (a, alert_trust::CERT, n.to_string()),
                Err(e) => {
                    return self.log(format!("dropped an alert from {}: {e:?}", short(&from)));
                }
            }
        } else if let Some(v) = bulletin::verify(&from, plain, &self.bulletin_keys) {
            // v1: signed, but no serial or expiry; the timestamp stands in
            // for both (valid for a day).
            let a = Alert {
                status: alert::status::ACTUAL,
                kind: alert::kind::ALERT,
                serial: v.timestamp,
                issued: v.timestamp,
                expires_min: 24 * 60,
                event: alert::event::INFO,
                severity: 1,
                reference: 0,
                area: None,
                text: v.text,
            };
            (a, alert_trust::LEGACY, String::new())
        } else {
            return self.log(format!("dropped an unverified message on {} from {}", bulletin::CHANNEL_NAME, short(&from)));
        };
        match self.alerts.guard.check(&from, &a, clock()) {
            Check::New => {}
            Check::Repeat => return,
            other => return self.log(format!("dropped alert {} from {}: {other:?}", a.serial, short(&from))),
        }
        self.alerts.save_guard();
        self.log(format!("verified alert {} from {} ({} hops)", a.serial, if name.is_empty() { short(&from) } else { name.clone() }, hops));
        self.show_alert(from, &a, hops, trust, &name);
    }

    fn show_alert(&mut self, issuer: Address, a: &Alert, hops: u8, trust: u8, name: &str) {
        let mut shown = format!("[alert {}] ", alert::event::NAMES.get(a.event as usize).unwrap_or(&"?")).into_bytes();
        shown.extend_from_slice(a.text);
        self.mqtt_channel(&bulletin::channel(), &issuer, hops, &shown);
        let ev = Event::Alert {
            issuer,
            serial: a.serial,
            issued: a.issued,
            meta: meta_of(a),
            hops,
            trust,
            name: heapless::Vec::from_slice(&name.as_bytes()[..name.len().min(24)]).unwrap_or_default(),
            text: text(a.text),
        };
        self.broadcast(&ev);
    }

    /// `Command::SendAlert`: sign with the issuer key and send on the
    /// bulletin channel (our certificate first, if it is due).
    pub(super) fn send_alert(&mut self, meta: &AlertMeta, body: &[u8]) -> Result<u32, (u8, &'static str)> {
        let Some(issuer) = self.alerts.issuer.as_ref() else {
            return Err((error_code::UNKNOWN_SENDER, "This host has no issuer key (--issuer-key)"));
        };
        let issuer_addr = issuer.address();
        let now = unix_now();
        // Serials only grow, also after a lost state file: never below the clock.
        let serial = (self.alerts.last_serial + 1).max(now);
        let a = Alert {
            status: meta.status.min(3),
            kind: meta.kind.min(2),
            serial,
            issued: now,
            expires_min: meta.expires_min.max(1),
            event: meta.event,
            severity: meta.severity.min(3),
            reference: meta.reference,
            area: meta.area.map(|(lat, lon, r)| Area { lat_e5: lat, lon_e5: lon, radius_100m: r }),
            text: body,
        };
        let mut plain = [0u8; 255];
        let n = alert::sign(issuer, &a, &mut plain[..200]).ok_or((error_code::TOO_LARGE, "The alert text is too long"))?;
        let cert_due = self.alerts.cert_sent.is_none_or(|t| t.elapsed() > CERT_EVERY);
        if let Some(c) = self.alerts.my_cert.clone().filter(|_| cert_due) {
            self.alerts.cert_sent = Some(Instant::now());
            self.send_bulletin(issuer_addr, &c);
        }
        if !self.send_bulletin(issuer_addr, &plain[..n]) {
            return Err((error_code::QUEUE_FULL, "Could not queue the alert"));
        }
        self.alerts.last_serial = serial;
        let _ = fs::write(self.alerts.dir.join("alert_serial.txt"), serial.to_string());
        // Shown here too, and remembered so our own echo is not shown twice.
        self.alerts.guard.check(&issuer_addr, &a, clock());
        self.alerts.save_guard();
        let (trust, name) = match self.alerts.certs.iter().find(|c| c.issuer.address() == issuer_addr) {
            Some(c) => (alert_trust::CERT, c.name.to_string()),
            None => (alert_trust::KEY, String::new()),
        };
        self.log(format!("issued alert {serial}"));
        self.show_alert(issuer_addr, &a, 0, trust, &name);
        Ok(serial)
    }

    /// Seals `plain` on the bulletin channel with `sender` in the seal and
    /// sends it.
    fn send_bulletin(&mut self, sender: Address, plain: &[u8]) -> bool {
        let channel = bulletin::channel();
        let mut nonce = [0u8; 8];
        OsRng.fill_bytes(&mut nonce);
        let mut ct = [0u8; 255];
        let Ok(n) = seal::seal_channel(&bulletin::channel_key(), &sender, nonce, plain, &mut ct) else {
            return false;
        };
        match self.router.send_channel_in(channel, 0, &ct[..n]) {
            Ok(actions) => {
                self.apply(actions.into_iter().collect());
                true
            }
            Err(_) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::{Receiver, channel};

    fn node(name: &str, keys: Vec<PublicIdentity>, roots: Vec<PublicIdentity>, issuer: Option<&Identity>) -> (Node, Receiver<Vec<u8>>) {
        let dir = std::env::temp_dir().join(format!("maarse-alert-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let mut c = file_interop_tests::cfg(dir.clone());
        c.bulletin_keys = keys;
        c.alert_roots = roots;
        if let Some(i) = issuer {
            let p = dir.join("issuer.key");
            fs::write(&p, i.to_secret_bytes()).unwrap();
            c.issuer_key = Some(p);
        }
        let mut n = Node::new(&c).unwrap();
        let (tx, rx) = channel();
        n.clients.insert(1, tx);
        (n, rx)
    }

    fn sealed(sender: &Address, plain: &[u8]) -> Vec<u8> {
        let mut ct = [0u8; 255];
        let m = seal::seal_channel(&bulletin::channel_key(), sender, [1; 8], plain, &mut ct).unwrap();
        ct[..m].to_vec()
    }

    fn alerts(rx: &Receiver<Vec<u8>>) -> Vec<(u32, u8, String)> {
        rx.try_iter()
            .filter_map(|f| Event::decode(&f[2..]).ok())
            .filter_map(|e| match e {
                Event::Alert { serial, trust, text, .. } => Some((serial, trust, String::from_utf8_lossy(&text).into_owned())),
                Event::ChannelReceived { text, .. } => panic!("bulletin traffic shown as chat: {:?}", String::from_utf8_lossy(&text)),
                _ => None,
            })
            .collect()
    }

    fn signed(issuer: &Identity, serial: u32, issued: u32, text: &'static [u8]) -> Vec<u8> {
        let a = Alert {
            status: 0,
            kind: 0,
            serial,
            issued,
            expires_min: 60,
            event: alert::event::FLOOD,
            severity: 2,
            reference: 0,
            area: None,
            text,
        };
        let mut b = [0u8; 255];
        let n = alert::sign(issuer, &a, &mut b).unwrap();
        b[..n].to_vec()
    }

    #[test]
    fn alerts_are_verified_once_and_replays_dropped() {
        let issuer = Identity::generate(&mut OsRng);
        let mallory = Identity::generate(&mut OsRng);
        let (mut b, rx) = node("rx", vec![issuer.public()], Vec::new(), None);
        let ch = bulletin::channel();
        assert!(b.channels.contains_key(&ch));
        let now = unix_now();
        let p = sealed(&issuer.address(), &signed(&issuer, 5, now, b"Flood"));
        b.on_deliver(PacketType::Channel, ch, &p, 3);
        assert_eq!(alerts(&rx), vec![(5, alert_trust::KEY, "Flood".to_string())]);
        // A repeat, an expired replay, and a forgery: nothing shown.
        b.on_deliver(PacketType::Channel, ch, &sealed(&issuer.address(), &signed(&issuer, 5, now, b"Flood")), 3);
        b.on_deliver(PacketType::Channel, ch, &sealed(&issuer.address(), &signed(&issuer, 4, now - 7200, b"Old")), 3);
        b.on_deliver(PacketType::Channel, ch, &sealed(&issuer.address(), &signed(&mallory, 6, now, b"Fake")), 3);
        b.on_deliver(PacketType::Channel, ch, &sealed(&mallory.address(), &signed(&mallory, 6, now, b"Fake")), 3);
        // Plain text claiming to be verified is not an alert either.
        b.on_deliver(PacketType::Channel, ch, &sealed(&mallory.address(), b"[Verified bulletin] Fake"), 3);
        assert!(alerts(&rx).is_empty());
    }

    #[test]
    fn certificates_from_a_root_name_the_issuer() {
        let root = Identity::generate(&mut OsRng);
        let issuer = Identity::generate(&mut OsRng);
        let cert = Cert {
            issuer: issuer.public(),
            not_after: unix_now() + 86_400,
            events: 0xffff,
            max_severity: 3,
            name: heapless::String::try_from("Beredskab Nord").unwrap(),
        };
        let mut cb = [0u8; 200];
        let n = alert::sign_cert(&root, &cert, &mut cb).unwrap();
        let (mut b, rx) = node("cert", Vec::new(), vec![root.public()], None);
        let ch = bulletin::channel();
        // The alert before the certificate is not trusted yet.
        b.on_deliver(PacketType::Channel, ch, &sealed(&issuer.address(), &signed(&issuer, 1, unix_now(), b"Early")), 2);
        assert!(alerts(&rx).is_empty());
        b.on_deliver(PacketType::Channel, ch, &sealed(&issuer.address(), &cb[..n]), 2);
        b.on_deliver(PacketType::Channel, ch, &sealed(&issuer.address(), &signed(&issuer, 2, unix_now(), b"Now")), 2);
        assert_eq!(alerts(&rx), vec![(2, alert_trust::CERT, "Now".to_string())]);
    }

    #[test]
    fn only_a_host_with_an_issuer_key_issues() {
        let issuer = Identity::generate(&mut OsRng);
        let (mut a, rx) = node("issuer", Vec::new(), Vec::new(), Some(&issuer));
        let (mut plain, _) = node("plain", Vec::new(), Vec::new(), None);
        let meta = AlertMeta { event: alert::event::WATER, severity: 1, expires_min: 120, ..Default::default() };
        assert!(plain.send_alert(&meta, b"Boil water").is_err());
        let s1 = a.send_alert(&meta, b"Boil water").unwrap();
        let s2 = a.send_alert(&meta, b"Boil water, update").unwrap();
        assert!(s2 > s1, "serials grow");
        assert_eq!(alerts(&rx).len(), 2);
        // Typing on the bulletin channel never produces a signed alert.
        assert!(a.channels.contains_key(&bulletin::channel()));
    }
}
