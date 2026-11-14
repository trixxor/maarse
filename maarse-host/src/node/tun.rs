//! IP-tunnel i noden: pakker fra adapteren sendes i stykker til den node,
//! hvis IPv6-adresse de er til; samlede pakker skrives til adapteren, eller
//! besvares (ping), når hosten ikke har nogen adapter.

use super::{Node, Pending, short};
use crate::tunnel;
use maarse_core::Address;
use std::sync::mpsc::Sender;
use std::time::Instant;

impl Node {
    pub fn my_ip(&self) -> [u8; 16] {
        tunnel::ip_of(&self.addr)
    }

    #[cfg_attr(not(feature = "tun"), allow(dead_code))]
    pub fn set_tun(&mut self, tx: Sender<Vec<u8>>) {
        self.tun_tx = Some(tx);
    }

    /// En pakke fra adapteren: find noden og send den i stykker (kun TCP).
    pub(super) fn on_tun_packet(&mut self, p: &[u8]) {
        let Some(dst) = tunnel::ipv6_dst(p) else { return };
        if dst == self.my_ip() || p.len() > tunnel::MTU {
            return;
        }
        let Some(dest) = self.known.keys().find(|a| tunnel::ip_of(a) == dst).copied() else {
            return; // ukendt adresse (multicast, fremmede net …)
        };
        self.tunnel_send(dest, p);
    }

    fn tunnel_send(&mut self, dest: Address, p: &[u8]) {
        if self.file_limit(&dest).via_lora {
            return; // IP-trafik kun over TCP hele vejen
        }
        self.tun_seq = self.tun_seq.wrapping_add(1);
        for f in tunnel::fragment(p, self.tun_seq) {
            if self.send_sealed(dest, &f, Pending::Control).is_err() {
                return;
            }
        }
    }

    pub(super) fn on_ip_fragment(&mut self, from: Address, data: &[u8]) {
        let Some(pkt) = self.reasm.add(from, data, Instant::now()) else { return };
        // Kun pakker fra afsenderens egen adresse til vores.
        if pkt.len() < 40 || pkt[8..24] != tunnel::ip_of(&from) || tunnel::ipv6_dst(&pkt) != Some(self.my_ip()) {
            self.log(format!("IP-pakke fra {} med forkert adresse smidt væk", short(&from)));
            return;
        }
        if let Some(tx) = &self.tun_tx {
            let _ = tx.send(pkt);
        } else if let Some(reply) = tunnel::echo_reply(&pkt, &self.my_ip()) {
            self.tunnel_send(from, &reply);
        }
    }
}
