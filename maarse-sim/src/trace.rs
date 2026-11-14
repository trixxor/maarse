//! Valgfrit hændelsesspor til visualisering (`--trace-json <fil|->`).
//!
//! Én JSON-linje pr. hændelse. Bruges af `maarse-lab` til at animere
//! simuleringen i browseren. Når sporet er slået fra, koster kaldene kun et
//! tjek af et flag. Formatet:
//!
//! ```text
//! {"ev":"setup","mode":"maarse","highway":false,"w":30000,"h":30000,"dur":900000,"nodes":[[x,y,"r"|"c"|"car"],...]}
//! {"ev":"links","l":[[a,b,snr],...]}            hørbare par (a<b), SNR i dB
//! {"t":ms,"ev":"tx","n":node,"id":tx,"end":ms,"air":µs,"p":"Announce"}
//! {"t":ms,"ev":"rx","id":tx,"ok":[...],"coll":[...]}
//! {"t":ms,"ev":"defer","n":node,"why":"duty"|"busy"}
//! {"t":ms,"ev":"send","msg":i,"from":a,"to":b}
//! {"t":ms,"ev":"deliver","msg":i,"n":node}
//! {"t":ms,"ev":"ack","msg":i}
//! {"t":ms,"ev":"move","n":node,"x":m,"y":m}
//! {"ev":"end"}
//! ```

use std::fmt::Write as _;
use std::io::Write;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

static ON: AtomicBool = AtomicBool::new(false);
static OUT: Mutex<Option<Box<dyn Write + Send>>> = Mutex::new(None);

/// Åbn sporet. `-` betyder stdout.
pub fn open(path: &str) {
    let w: Box<dyn Write + Send> = if path == "-" {
        Box::new(std::io::BufWriter::new(std::io::stdout()))
    } else {
        Box::new(std::io::BufWriter::new(
            std::fs::File::create(path).expect("--trace-json: kan ikke oprette fil"),
        ))
    };
    *OUT.lock().unwrap() = Some(w);
    ON.store(true, Ordering::Relaxed);
}

#[inline]
pub fn on() -> bool {
    ON.load(Ordering::Relaxed)
}

fn line(s: &str) {
    if let Some(w) = OUT.lock().unwrap().as_mut() {
        let _ = w.write_all(s.as_bytes());
        let _ = w.write_all(b"\n");
    }
}

pub fn finish() {
    if !on() {
        return;
    }
    line("{\"ev\":\"end\"}");
    if let Some(w) = OUT.lock().unwrap().as_mut() {
        let _ = w.flush();
    }
}

/// Noder: (x, y, rolle). Links: (a, b, snr).
pub fn setup(
    mode: &str,
    highway: bool,
    size: (f64, f64),
    dur_ms: u64,
    nodes: &[(f64, f64, &str)],
    links: &[(usize, usize, f64)],
) {
    if !on() {
        return;
    }
    let mut s = format!(
        "{{\"ev\":\"setup\",\"mode\":\"{mode}\",\"highway\":{highway},\"w\":{:.0},\"h\":{:.0},\"dur\":{dur_ms},\"nodes\":[",
        size.0, size.1
    );
    for (i, (x, y, r)) in nodes.iter().enumerate() {
        let _ = write!(s, "{}[{x:.0},{y:.0},\"{r}\"]", if i > 0 { "," } else { "" });
    }
    s.push_str("]}");
    line(&s);
    let mut s = String::from("{\"ev\":\"links\",\"l\":[");
    for (i, (a, b, snr)) in links.iter().enumerate() {
        let _ = write!(s, "{}[{a},{b},{snr:.1}]", if i > 0 { "," } else { "" });
    }
    s.push_str("]}");
    line(&s);
}

pub fn tx(t: u64, node: usize, id: usize, end: u64, air_us: u32, ptype: &str) {
    if on() {
        line(&format!(
            "{{\"t\":{t},\"ev\":\"tx\",\"n\":{node},\"id\":{id},\"end\":{end},\"air\":{air_us},\"p\":\"{ptype}\"}}"
        ));
    }
}

pub fn defer(t: u64, node: usize, why: &str) {
    if on() {
        line(&format!("{{\"t\":{t},\"ev\":\"defer\",\"n\":{node},\"why\":\"{why}\"}}"));
    }
}

pub fn send(t: u64, msg: usize, from: usize, to: usize) {
    if on() {
        line(&format!("{{\"t\":{t},\"ev\":\"send\",\"msg\":{msg},\"from\":{from},\"to\":{to}}}"));
    }
}

pub fn deliver(t: u64, msg: usize, node: usize) {
    if on() {
        line(&format!("{{\"t\":{t},\"ev\":\"deliver\",\"msg\":{msg},\"n\":{node}}}"));
    }
}

pub fn ack(t: u64, msg: usize) {
    if on() {
        line(&format!("{{\"t\":{t},\"ev\":\"ack\",\"msg\":{msg}}}"));
    }
}

pub fn moved(t: u64, node: usize, pos: (f64, f64)) {
    if on() {
        line(&format!(
            "{{\"t\":{t},\"ev\":\"move\",\"n\":{node},\"x\":{:.0},\"y\":{:.0}}}",
            pos.0, pos.1
        ));
    }
}

/// Samler modtagere af én udsendelse.
#[derive(Default)]
pub struct Rx {
    ok: Vec<usize>,
    coll: Vec<usize>,
}

impl Rx {
    pub fn ok(&mut self, n: usize) {
        if on() {
            self.ok.push(n);
        }
    }
    pub fn coll(&mut self, n: usize) {
        if on() {
            self.coll.push(n);
        }
    }
    pub fn emit(self, t: u64, id: usize) {
        if !on() {
            return;
        }
        let j = |v: &[usize]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(",");
        line(&format!(
            "{{\"t\":{t},\"ev\":\"rx\",\"id\":{id},\"ok\":[{}],\"coll\":[{}]}}",
            j(&self.ok),
            j(&self.coll)
        ));
    }
}
