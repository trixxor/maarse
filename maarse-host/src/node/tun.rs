//! IP-tunnel i noden (se `crate::tunnel`): pakker fra adapteren sendes som
//! datagrammer i en session; hele pakker skrives til adapteren eller
//! besvares (ping), når hosten ikke har nogen adapter.

use super::{Node, Pending, short, token};
use crate::tunnel::{self, TunOut};
use maarse_core::{Address, PacketType};
use rand_core::{OsRng, RngCore};
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

    /// En pakke fra adapteren: find noden og send den gennem tunnelen.
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
        if self.file_limit(&dest).via_lora && !self.tun_lora {
            return; // IP over LoRa kun med --tun-lora
        }
        let me = self.addr;
        let outs = self.tunnel.send(&me, dest, p, Instant::now(), &mut |b| OsRng.fill_bytes(b));
        self.tun_apply(outs);
    }

    fn tun_apply(&mut self, outs: Vec<TunOut>) {
        for o in outs {
            match o {
                TunOut::Control(dest, c) => {
                    let mut plain = vec![maarse_core::content::IP];
                    plain.extend_from_slice(&c);
                    let _ = self.send_sealed(dest, &plain, Pending::Control);
                }
                TunOut::Datagram(dest, dg) => {
                    let now = self.now_ms();
                    if let Ok((_, actions)) = self.router.send_typed(PacketType::Datagram, dest, &dg, token(), now) {
                        self.apply(actions.into_iter().collect());
                    }
                }
                TunOut::Packet(from, pkt) => self.on_tun_whole(from, pkt),
            }
        }
    }

    pub(super) fn on_tun_control(&mut self, from: Address, data: &[u8]) {
        let me = self.addr;
        let outs = self.tunnel.on_control(from, data, &me, Instant::now());
        self.tun_apply(outs);
    }

    pub(super) fn on_tun_datagram(&mut self, dg: &[u8]) {
        let me = self.addr;
        let outs = self.tunnel.on_datagram(dg, &me, Instant::now());
        self.tun_apply(outs);
    }

    pub(super) fn tun_tick(&mut self) {
        let outs = self.tunnel.tick(Instant::now());
        self.tun_apply(outs);
    }

    fn on_tun_whole(&mut self, from: Address, pkt: Vec<u8>) {
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
