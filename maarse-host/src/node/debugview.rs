//! The snapshot for the debug view (see debug.rs): every table the host
//! keeps, as JSON, rebuilt every 2 s.

use super::*;
use crate::debug::hex;
use serde_json::json;

impl Node {
    fn name_of(&self, a: &Address) -> String {
        self.ext.names.get(a).map(|(n, _)| String::from_utf8_lossy(n).into_owned()).unwrap_or_default()
    }

    pub(super) fn debug_snapshot(&self) {
        let now = self.now_ms();
        let st = &self.router.stats;
        let links: Vec<_> = self
            .links
            .iter()
            .map(|(id, (name, _))| {
                let c = self.link_stats.get(id).copied().unwrap_or_default();
                let kind = if name.starts_with("trunk ") {
                    "LoRa trunk"
                } else if crate::extra_links::is_backbone(name) {
                    "backbone"
                } else {
                    "radio"
                };
                let regions = self.link_regions.get(id).map(|(s, r)| json!({"strict": s, "regions": r}));
                json!({"id": id, "name": name, "kind": kind, "rx_packets": c[0], "tx_packets": c[1], "rx_bytes": c[2], "tx_bytes": c[3], "regions": regions, "caps": self.link_caps.get(id)})
            })
            .collect();
        let routes: Vec<_> = self
            .router
            .routes()
            .iter()
            .map(|r| {
                json!({
                    "dest": hex(&r.dest.0),
                    "name": self.name_of(&r.dest),
                    "path": r.path.iter().map(|x| format!("{x:04x}")).collect::<Vec<_>>(),
                    "cost": r.cost,
                    "age_s": now.saturating_sub(r.learned_ms) / 1000,
                    "last_used_s": now.saturating_sub(r.last_seen_ms) / 1000,
                    "confirmed_s": (r.confirmed_ms > 0).then(|| now.saturating_sub(r.confirmed_ms) / 1000),
                })
            })
            .collect();
        let neighbors: Vec<_> = self
            .router
            .neighbors
            .iter()
            .map(|n| {
                json!({
                    "id": format!("{:04x}", n.id),
                    "addr": n.addr.map(|a| hex(&a.0)),
                    "name": n.addr.map(|a| self.name_of(&a)).unwrap_or_default(),
                    "snr": n.snr,
                    "rssi": n.rssi,
                    "heard_s": now.saturating_sub(n.last_ms) / 1000,
                    "count": n.count,
                    "role": n.role.map(|r| format!("{r:?}")),
                    "hears_repeater": n.hears_repeater,
                    "mail_class": n.mail_class,
                })
            })
            .collect();
        let contacts: Vec<_> = self
            .known
            .keys()
            .take(500)
            .map(|a| {
                let via = self.heard_via.get(a).map(|(l, tcp, hops)| {
                    json!({"link": self.links.get(l).map(|(n, _)| n.clone()), "tcp": tcp, "hops": hops})
                });
                json!({"addr": hex(&a.0), "name": self.name_of(a), "via": via, "route": self.router.has_route(a)})
            })
            .collect();
        let dht_table: Vec<_> = self
            .dht
            .table
            .all()
            .iter()
            .map(|c| {
                let k = self.caps_of.get(&c.id);
                json!({"id": hex(&c.id.0), "ep": c.ep, "caps": k.map(|(c, _)| c), "caps_age_s": k.map(|(_, t)| t.elapsed().as_secs())})
            })
            .collect();
        let dht_records: Vec<_> = self
            .dht
            .store
            .all()
            .iter()
            .map(|r| json!({"dest": hex(&r.dest.0), "name": self.name_of(&r.dest), "gw": hex(&r.gw.0), "ep": r.ep, "hops": r.hops, "age_s": r.age_s}))
            .collect();
        let mailbox: Vec<_> = self
            .mailbox
            .iter()
            .map(|(rcpt, m)| {
                let oldest = m.values().filter_map(|x| x.stored.elapsed().ok()).map(|d| d.as_secs()).max().unwrap_or(0);
                json!({
                    "recipient": hex(&rcpt.0),
                    "name": self.name_of(rcpt),
                    "count": m.len(),
                    "bytes": m.values().map(|x| x.sealed.len()).sum::<usize>(),
                    "oldest_s": oldest,
                    "replicas": self.mail_replicas(rcpt).iter().map(|c| c.ep.clone()).collect::<Vec<_>>(),
                })
            })
            .collect();
        let trunks: Vec<_> = self
            .links
            .iter()
            .filter(|(_, (n, _))| n.starts_with("trunk "))
            .map(|(id, (n, _))| {
                json!({
                    "link": n,
                    "peers": self.trunk_peers.get(id).map_or(0, |p| p.len()),
                    "summary_age_s": self.trunk_bloom.get(id).map(|(_, t)| t.elapsed().as_secs()),
                })
            })
            .collect();
        let snap = json!({
            "me": {
                "addr": hex(&self.addr.0),
                "role": format!("{:?}", self.router.role),
                "uptime_s": self.start.elapsed().as_secs(),
                "server_mode": self.backbone,
                "public_ep": self.dht.public_ep,
                "caps": self.my_caps(),
                "mail_bytes": self.mail_bytes(),
                "mail_quota": self.mail_quota,
            },
            "counters": {
                "packets_in": self.counters.packets_in, "packets_out": self.counters.packets_out,
                "bytes_in": self.counters.bytes_in, "bytes_out": self.counters.bytes_out,
                "deposits": self.counters.deposits, "mail_delivered": self.counters.mail_delivered,
                "errors": self.counters.errors, "last_snr": self.counters.last_snr,
            },
            "router": {
                "rx_packets": st.rx_packets, "rx_invalid": st.rx_invalid, "rx_duplicates": st.rx_duplicates,
                "forwarded": st.forwarded, "originated": st.originated, "delivered": st.delivered, "acked": st.acked,
                "routes_learned": st.routes_learned, "path_requests": st.path_requests, "hop_retries": st.hop_retries,
                "floods_cancelled": st.floods_cancelled, "bad_stamps": st.bad_stamps, "local_repairs": st.local_repairs,
                "hellos": st.hellos, "key_requests": st.key_requests, "carried": self.router.carried_count(),
            },
            "links": links,
            "routes": routes,
            "neighbors": neighbors,
            "contacts": contacts,
            "contacts_total": self.known.len(),
            "dht": {
                "table": dht_table, "records": dht_records, "seeds": self.dht.seeds, "peers": self.dht.peers.iter().collect::<Vec<_>>(),
                "reachable": self.dht.heard.values().filter(|h| h.alive(std::time::Instant::now())).count(), "lookups": self.dht.lookups, "found": self.dht.found,
            },
            "mailbox": mailbox,
            "trunks": trunks,
            "clients": self.clients.len(),
        });
        crate::debug::set_snapshot(snap.to_string());
    }
}
