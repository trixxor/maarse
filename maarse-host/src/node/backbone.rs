//! The backbones this host is linked to, for the screens' backbone menu.
//!
//! * `0x39 GetBackbones` (no content). The answer is one `0xBA Backbone` per
//!   backbone link, then `0xBA` with link 0 (end of list):
//!   link (u16) | flags (u8: 1 server, 2 radio, 4 post office, 8 relay,
//!   16 call test) | up for (u32 s) | address (16, zero when not heard) |
//!   version (u8 len + text) | name (u8 len + text) | n (u8) | regions (u16 BE)
//!
//! The address is the node heard at 0 hops over that link: the server
//! itself. The screens use it for a route trace and for the call test (see
//! `calltest.rs`), which answers calls with music, echo or a speed test.

use super::Node;
use crate::ClientId;
use maarse_core::Address;

pub const CMD_GET_BACKBONES: u8 = 0x39;
pub const EV_BACKBONE: u8 = 0xBA;

fn frame(body: &[u8]) -> Vec<u8> {
    let mut v = ((body.len() + 1) as u16).to_le_bytes().to_vec();
    v.push(EV_BACKBONE);
    v.extend_from_slice(body);
    v
}

fn put_str(b: &mut Vec<u8>, s: &str) {
    let s = &s.as_bytes()[..s.len().min(64)];
    b.push(s.len() as u8);
    b.extend_from_slice(s);
}

impl Node {
    pub(super) fn backbones(&mut self, id: ClientId) {
        let mut links: Vec<_> = self
            .links
            .iter()
            .filter(|(_, (n, _))| crate::extra_links::is_backbone(n))
            .map(|(l, (n, _))| (*l, n.clone()))
            .collect();
        links.sort_by_key(|(l, _)| *l);
        for (link, name) in links {
            let caps = self.link_caps.get(&link);
            let addr = self
                .heard_via
                .iter()
                .find(|(_, (l, _, hops))| *l == link && *hops == 0)
                .map(|(a, _)| *a)
                .unwrap_or_default();
            let up = self.link_up.get(&link).map_or(0, |t| t.elapsed().as_secs() as u32);
            let mut b = (link as u16).to_le_bytes().to_vec();
            let flags = caps.map_or(0, |c| c.server as u8 | (c.radio as u8) << 1 | (c.post_office as u8) << 2 | (c.relay as u8) << 3)
                | if caps.is_some_and(|c| c.call_test) { 16 } else { 0 };
            b.push(flags);
            b.extend_from_slice(&up.to_le_bytes());
            b.extend_from_slice(&addr.0);
            put_str(&mut b, caps.map_or("", |c| c.version.as_str()));
            put_str(&mut b, &name);
            let regions: Vec<u16> = caps.map(|c| c.regions.clone()).unwrap_or_default();
            b.push(regions.len().min(8) as u8);
            for r in regions.iter().take(8) {
                b.extend_from_slice(&r.to_be_bytes());
            }
            self.send_backbone_frame(id, frame(&b));
        }
        let mut end = 0u16.to_le_bytes().to_vec();
        end.extend_from_slice(&[0; 5]);
        end.extend_from_slice(&Address::default().0);
        end.extend_from_slice(&[0, 0, 0]);
        self.send_backbone_frame(id, frame(&end));
    }

    fn send_backbone_frame(&self, id: ClientId, f: Vec<u8>) {
        if let Some(tx) = self.clients.get(&id) {
            let _ = tx.send(f);
        }
    }
}
