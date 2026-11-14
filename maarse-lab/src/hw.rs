//! Live hardwarevisning: læser status fra rigtige boards over
//! companion-protokollen. Kun porte givet med `--port` åbnes.

use crate::{broadcast, cfg};
use maarse_core::companion::{Command, Event, FrameDecoder};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

static PAUSE: AtomicBool = AtomicBool::new(false);
static LAST: Mutex<Option<HashMap<String, Value>>> = Mutex::new(None);

/// Slip portene, mens en hardwaretest bruger dem.
pub fn pause(on: bool) {
    PAUSE.store(on, Ordering::SeqCst);
    if on {
        thread::sleep(Duration::from_millis(600));
    }
}

pub fn snapshot() -> Vec<Value> {
    LAST.lock()
        .unwrap()
        .as_ref()
        .map(|m| m.values().cloned().collect())
        .unwrap_or_default()
}

fn publish(port: &str, f: impl FnOnce(&mut Value)) {
    let v = {
        let mut g = LAST.lock().unwrap();
        let m = g.get_or_insert_with(HashMap::new);
        let e = m
            .entry(port.to_string())
            .or_insert_with(|| json!({"type":"hw","port":port,"status":"starter","stats":{},"log":[]}));
        f(e);
        e.clone()
    };
    broadcast(&v);
}

pub fn spawn_all() {
    for p in cfg().ports.clone() {
        thread::spawn(move || poll(p));
    }
}

fn poll(port: String) {
    loop {
        if PAUSE.load(Ordering::SeqCst) {
            publish(&port, |e| e["status"] = json!("på pause (hardwaretest kører)"));
            while PAUSE.load(Ordering::SeqCst) {
                thread::sleep(Duration::from_millis(200));
            }
        }
        match serialport::new(&port, 115_200)
            .timeout(Duration::from_millis(100))
            .open()
        {
            Ok(p) => {
                publish(&port, |e| e["status"] = json!("forbundet"));
                session(&port, p);
            }
            Err(err) => publish(&port, |e| e["status"] = json!(format!("kan ikke åbne: {err}"))),
        }
        thread::sleep(Duration::from_secs(2));
    }
}

fn session(port: &str, mut p: Box<dyn serialport::SerialPort>) {
    let send = |p: &mut Box<dyn serialport::SerialPort>, c: Command| p.write_all(&c.encode()).is_ok();
    if !send(&mut p, Command::GetInfo) || !send(&mut p, Command::GetRadio) {
        return;
    }
    let mut dec = FrameDecoder::new();
    let mut buf = [0u8; 512];
    let mut last_poll = Instant::now() - Duration::from_secs(10);
    let mut last_rx = Instant::now();
    loop {
        if PAUSE.load(Ordering::SeqCst) {
            return;
        }
        if last_poll.elapsed() > Duration::from_millis(1500) {
            if !send(&mut p, Command::GetStats) {
                return;
            }
            last_poll = Instant::now();
        }
        if last_rx.elapsed() > Duration::from_secs(10) {
            publish(port, |e| e["status"] = json!("intet svar (er det en Må:rse-node?)"));
        }
        match p.read(&mut buf) {
            Ok(n) if n > 0 => {
                dec.push(&buf[..n]);
                while let Some(f) = dec.next_frame() {
                    last_rx = Instant::now();
                    if let Ok(ev) = Event::decode(&f) {
                        on_event(port, ev);
                    }
                }
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(_) => return,
        }
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn on_event(port: &str, ev: Event) {
    match ev {
        Event::Info { addr, repeater, version, .. } => publish(port, |e| {
            e["status"] = json!("forbundet");
            e["addr"] = json!(hex(&addr.0));
            e["repeater"] = json!(repeater);
            e["version"] = json!(version);
        }),
        Event::Radio(r) => publish(port, |e| {
            e["radio"] = json!({"freq_hz":r.freq_hz,"bw_hz":r.bw_hz,"sf":r.sf,"cr":r.cr,
                                 "dbm":r.tx_power_dbm,"duty_permille":r.duty_permille});
        }),
        Event::Stats { items } => publish(port, |e| {
            e["status"] = json!("forbundet");
            for (k, v) in items.iter() {
                e["stats"][k.to_string()] = json!(v);
            }
            e["updated"] = json!(std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0));
        }),
        Event::Log { text } => publish(port, |e| {
            let t = String::from_utf8_lossy(&text).to_string();
            if let Some(a) = e["log"].as_array_mut() {
                a.push(json!(t));
                let n = a.len();
                if n > 30 {
                    a.drain(..n - 30);
                }
            }
        }),
        Event::Heard { addr, hops, .. } => publish(port, |e| {
            e["heard"][hex(&addr.0[..4])] = json!(hops);
        }),
        _ => {}
    }
}
