//! MQTT bridge: publishes received telemetry, positions and channel messages as JSON
//! to an MQTT broker, with Home Assistant MQTT discovery for the nodes'
//! sensors.
//!
//! A small MQTT 3.1.1 client (only CONNECT, PUBLISH with QoS 0 and PINGREQ)
//! written by hand, so the host does not pull in an async runtime.
//! The client runs in its own thread and reconnects by itself.
//!
//! Topics (`<prefix>` defaults to `maarse`):
//!
//! ```text
//! <prefix>/status                       "online" / "offline" (will, retained)
//! <prefix>/node/<address>/telemetry     {"battery_mv":3870, ...} (retained)
//! <prefix>/node/<address>/position      {"lat":56.155, "lon":10.215, ...} (retained)
//! <prefix>/channel/<channel>            {"from":"…", "text":"…", ...}
//! homeassistant/sensor/maarse_<short>/<key>/config   discovery (retained)
//! ```

use maarse_core::Address;
use maarse_core::appdata::{Position, Precision, Telemetry};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

const KEEPALIVE_S: u16 = 60;

#[derive(Clone, Debug)]
pub struct MqttConfig {
    /// "host:port"
    pub addr: String,
    pub prefix: String,
    pub user: Option<String>,
    pub pass: Option<String>,
}

pub struct Msg {
    pub topic: String,
    pub payload: String,
    pub retain: bool,
}

/// Handle to the MQTT thread.
pub struct Mqtt {
    tx: Sender<Msg>,
    pub prefix: String,
    /// Nodes we have sent discovery for.
    discovered: std::collections::HashSet<Address>,
}

impl Mqtt {
    pub fn spawn(cfg: MqttConfig, client_id: String) -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        let prefix = cfg.prefix.clone();
        std::thread::spawn(move || run(cfg, client_id, rx));
        Mqtt { tx, prefix, discovered: Default::default() }
    }

    fn send(&self, topic: String, payload: String, retain: bool) {
        let _ = self.tx.send(Msg { topic, payload, retain });
    }

    /// Telemetry from a node (and discovery the first time).
    pub fn telemetry(&mut self, addr: &Address, hops: u8, t: &Telemetry) {
        if self.discovered.insert(*addr) {
            for (topic, payload) in discovery(&self.prefix, addr) {
                self.send(topic, payload, true);
            }
        }
        self.send(node_topic(&self.prefix, addr, "telemetry"), telemetry_json(addr, hops, t), true);
    }

    pub fn position(&self, addr: &Address, hops: u8, p: &Position) {
        self.send(node_topic(&self.prefix, addr, "position"), position_json(addr, hops, p), true);
    }

    pub fn channel(&self, channel: &str, from: &Address, hops: u8, text: &str) {
        let name: String = channel
            .trim_start_matches('#')
            .chars()
            .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
            .collect();
        self.send(
            format!("{}/channel/{}", self.prefix, name),
            channel_json(channel, from, hops, text),
            false,
        );
    }
}

pub fn hex(a: &Address) -> String {
    a.0.iter().map(|b| format!("{b:02x}")).collect()
}

fn node_topic(prefix: &str, a: &Address, what: &str) -> String {
    format!("{prefix}/node/{}/{what}", hex(a))
}

/// JSON string with quotes and escaping.
pub fn json_str(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

pub fn telemetry_json(addr: &Address, hops: u8, t: &Telemetry) -> String {
    let mut s = format!("{{\"node\":{},\"hops\":{hops}", json_str(&hex(addr)));
    if t.battery_mv > 0 {
        s += &format!(
            ",\"battery_mv\":{},\"battery_v\":{:.3},\"battery_percent\":{}",
            t.battery_mv,
            t.battery_mv as f32 / 1000.0,
            maarse_core::battery::percent(t.battery_mv)
        );
    }
    s += &format!(",\"ext_power\":{},\"energy\":{},\"uptime_s\":{}", t.ext_power, t.energy, t.uptime_s);
    if let Some(d) = t.temp_dc {
        s += &format!(",\"temperature_c\":{:.1}", d as f32 / 10.0);
    }
    s.push('}');
    s
}

pub fn position_json(addr: &Address, hops: u8, p: &Position) -> String {
    let prec = match p.precision {
        Precision::Km10 => "10km",
        Precision::Km1 => "1km",
        Precision::Precise => "precise",
        Precision::Off => "off",
    };
    format!(
        "{{\"node\":{},\"hops\":{hops},\"lat\":{:.5},\"lon\":{:.5},\"precision\":{}}}",
        json_str(&hex(addr)),
        p.lat_e7 as f64 / 1e7,
        p.lon_e7 as f64 / 1e7,
        json_str(prec)
    )
}

pub fn channel_json(channel: &str, from: &Address, hops: u8, text: &str) -> String {
    format!(
        "{{\"channel\":{},\"from\":{},\"hops\":{hops},\"text\":{}}}",
        json_str(channel),
        json_str(&hex(from)),
        json_str(text)
    )
}

/// Home Assistant discovery: one sensor per value in the telemetry.
pub fn discovery(prefix: &str, addr: &Address) -> Vec<(String, String)> {
    let full = hex(addr);
    let short = &full[..8];
    let state = node_topic(prefix, addr, "telemetry");
    // (key, name, unit, device_class, template)
    let sensors: [(&str, &str, &str, &str, &str); 6] = [
        ("battery_v", "Battery voltage", "V", "voltage", "{{ value_json.battery_v }}"),
        ("battery", "Battery", "%", "battery", "{{ value_json.battery_percent }}"),
        ("energy", "Energy status", "", "", "{{ value_json.energy }}"),
        ("uptime", "Uptime", "s", "duration", "{{ value_json.uptime_s }}"),
        ("temperature", "Temperature", "°C", "temperature", "{{ value_json.temperature_c }}"),
        ("hops", "Hops", "", "", "{{ value_json.hops }}"),
    ];
    sensors
        .iter()
        .map(|(key, name, unit, class, tpl)| {
            let mut p = format!(
                "{{\"name\":{},\"unique_id\":{},\"object_id\":{},\"state_topic\":{},\"value_template\":{},\
                 \"availability_topic\":{},\"state_class\":\"measurement\"",
                json_str(name),
                json_str(&format!("maarse_{full}_{key}")),
                json_str(&format!("maarse_{short}_{key}")),
                json_str(&state),
                json_str(tpl),
                json_str(&format!("{prefix}/status")),
            );
            if !unit.is_empty() {
                p += &format!(",\"unit_of_measurement\":{}", json_str(unit));
            }
            if !class.is_empty() {
                p += &format!(",\"device_class\":{}", json_str(class));
            }
            p += &format!(
                ",\"device\":{{\"identifiers\":[{}],\"name\":{},\"manufacturer\":\"Må:rse\",\"model\":\"Må:rse-node\"}}}}",
                json_str(&format!("maarse_{full}")),
                json_str(&format!("Må:rse {short}")),
            );
            (format!("homeassistant/sensor/maarse_{short}/{key}/config"), p)
        })
        .collect()
}

// ---------- MQTT 3.1.1 ----------

fn remaining_length(mut n: usize, out: &mut Vec<u8>) {
    loop {
        let mut b = (n % 128) as u8;
        n /= 128;
        if n > 0 {
            b |= 0x80;
        }
        out.push(b);
        if n == 0 {
            break;
        }
    }
}

fn mqtt_str(s: &[u8], out: &mut Vec<u8>) {
    out.extend_from_slice(&(s.len() as u16).to_be_bytes());
    out.extend_from_slice(s);
}

/// CONNECT with clean session, will `<prefix>/status` = "offline" (retained)
/// and optional user name/password.
pub fn connect_packet(client_id: &str, cfg: &MqttConfig) -> Vec<u8> {
    let mut v = Vec::new();
    mqtt_str(b"MQTT", &mut v);
    v.push(4); // protocol level 3.1.1
    let mut flags = 0x02 | 0x04 | 0x20; // clean session, will, will retain
    if cfg.user.is_some() {
        flags |= 0x80;
    }
    if cfg.pass.is_some() {
        flags |= 0x40;
    }
    v.push(flags);
    v.extend_from_slice(&KEEPALIVE_S.to_be_bytes());
    mqtt_str(client_id.as_bytes(), &mut v);
    mqtt_str(format!("{}/status", cfg.prefix).as_bytes(), &mut v);
    mqtt_str(b"offline", &mut v);
    if let Some(u) = &cfg.user {
        mqtt_str(u.as_bytes(), &mut v);
    }
    if let Some(p) = &cfg.pass {
        mqtt_str(p.as_bytes(), &mut v);
    }
    let mut out = vec![0x10];
    remaining_length(v.len(), &mut out);
    out.extend_from_slice(&v);
    out
}

pub fn publish_packet(topic: &str, payload: &[u8], retain: bool) -> Vec<u8> {
    let mut v = Vec::new();
    mqtt_str(topic.as_bytes(), &mut v);
    v.extend_from_slice(payload);
    let mut out = vec![0x30 | retain as u8];
    remaining_length(v.len(), &mut out);
    out.extend_from_slice(&v);
    out
}

/// Decodes a PUBLISH packet (for tests and the mock broker): (topic, data, retain).
#[cfg(test)]
pub fn parse_publish(b: &[u8]) -> Option<(String, Vec<u8>, bool, usize)> {
    if b.first()? & 0xF0 != 0x30 {
        return None;
    }
    let (len, n) = read_remaining(&b[1..])?;
    let body = b.get(1 + n..1 + n + len)?;
    let tl = u16::from_be_bytes([*body.first()?, *body.get(1)?]) as usize;
    let topic = String::from_utf8(body.get(2..2 + tl)?.to_vec()).ok()?;
    Some((topic, body[2 + tl..].to_vec(), b[0] & 1 != 0, 1 + n + len))
}

#[cfg(test)]
fn read_remaining(b: &[u8]) -> Option<(usize, usize)> {
    let mut v = 0usize;
    for (i, &x) in b.iter().take(4).enumerate() {
        v |= ((x & 0x7f) as usize) << (7 * i);
        if x & 0x80 == 0 {
            return Some((v, i + 1));
        }
    }
    None
}

fn session(cfg: &MqttConfig, client_id: &str, rx: &Receiver<Msg>) -> std::io::Result<bool> {
    let mut s = TcpStream::connect(&cfg.addr)?;
    s.set_nodelay(true)?;
    s.set_read_timeout(Some(Duration::from_secs(10)))?;
    s.write_all(&connect_packet(client_id, cfg))?;
    let mut ack = [0u8; 4];
    s.read_exact(&mut ack)?;
    if ack[0] != 0x20 || ack[3] != 0 {
        eprintln!("MQTT: the broker rejected the connection (code {})", ack[3]);
        return Ok(false);
    }
    println!("  MQTT:     connected to {}", cfg.addr);
    s.write_all(&publish_packet(&format!("{}/status", cfg.prefix), b"online", true))?;
    // Replies from the broker (PINGRESP) are read and discarded in a thread.
    let mut r = s.try_clone()?;
    r.set_read_timeout(None)?;
    std::thread::spawn(move || {
        let mut b = [0u8; 256];
        while matches!(r.read(&mut b), Ok(n) if n > 0) {}
    });
    let mut last = Instant::now();
    loop {
        match rx.recv_timeout(Duration::from_secs(KEEPALIVE_S as u64 / 2)) {
            Ok(m) => s.write_all(&publish_packet(&m.topic, m.payload.as_bytes(), m.retain))?,
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                let _ = s.write_all(&[0xE0, 0]); // DISCONNECT
                return Ok(true);
            }
        }
        if last.elapsed() >= Duration::from_secs(KEEPALIVE_S as u64 / 2) {
            s.write_all(&[0xC0, 0])?; // PINGREQ
            last = Instant::now();
        }
    }
}

fn run(cfg: MqttConfig, client_id: String, rx: Receiver<Msg>) {
    loop {
        match session(&cfg, &client_id, &rx) {
            Ok(true) => return,
            Ok(false) => {}
            Err(e) => eprintln!("MQTT: {}: {e} (retrying in 10 s)", cfg.addr),
        }
        std::thread::sleep(Duration::from_secs(10));
        // Messages that arrived while we were down are discarded (QoS 0).
        while rx.try_recv().is_ok() {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    fn cfg(addr: String) -> MqttConfig {
        MqttConfig { addr, prefix: "maarse".into(), user: Some("u".into()), pass: Some("p".into()) }
    }

    #[test]
    fn packets() {
        let p = publish_packet("a/b", b"hi!", true);
        assert_eq!(p, [0x31, 8, 0, 3, b'a', b'/', b'b', b'h', b'i', b'!']);
        assert_eq!(parse_publish(&p), Some(("a/b".into(), b"hi!".to_vec(), true, p.len())));
        let big = publish_packet("t", &[0u8; 200], false);
        assert_eq!(&big[..3], &[0x30, 203 & 0x7f | 0x80, 1]);
        assert_eq!(parse_publish(&big).unwrap().1.len(), 200);
        let c = connect_packet("maarse-1", &cfg("x".into()));
        assert_eq!(c[0], 0x10);
        assert_eq!(&c[2..9], &[0, 4, b'M', b'Q', b'T', b'T', 4]);
        assert_eq!(c[9], 0x80 | 0x40 | 0x20 | 0x04 | 0x02);
    }

    #[test]
    fn json_payloads() {
        let a = Address([0xab; 16]);
        let t = Telemetry { battery_mv: 3870, ext_power: false, energy: 2, uptime_s: 61, temp_dc: Some(215) };
        let j = telemetry_json(&a, 1, &t);
        assert_eq!(
            j,
            format!(
                "{{\"node\":\"{}\",\"hops\":1,\"battery_mv\":3870,\"battery_v\":3.870,\"battery_percent\":70,\
                 \"ext_power\":false,\"energy\":2,\"uptime_s\":61,\"temperature_c\":21.5}}",
                "ab".repeat(16)
            )
        );
        let j = telemetry_json(&a, 0, &Telemetry { ext_power: true, energy: 3, ..Default::default() });
        assert!(!j.contains("battery"));
        let p = Position { precision: Precision::Km1, lat_e7: 561_550_000, lon_e7: 102_150_000 };
        assert!(position_json(&a, 2, &p).contains("\"lat\":56.15500,\"lon\":10.21500,\"precision\":\"1km\""));
        let c = channel_json("#aarhus", &a, 3, "hi \"world\"\n");
        assert!(c.ends_with("\"text\":\"hi \\\"world\\\"\\n\"}"));
        assert_eq!(json_str("\u{1}"), "\"\\u0001\"");
    }

    #[test]
    fn home_assistant_discovery() {
        let a = Address([0x12; 16]);
        let d = discovery("maarse", &a);
        assert_eq!(d.len(), 6);
        let (topic, p) = &d[0];
        assert_eq!(topic, "homeassistant/sensor/maarse_12121212/battery_v/config");
        assert!(p.contains(&format!("\"state_topic\":\"maarse/node/{}/telemetry\"", "12".repeat(16))));
        assert!(p.contains("\"unit_of_measurement\":\"V\""));
        assert!(p.contains("\"device_class\":\"voltage\""));
        assert!(p.contains("\"availability_topic\":\"maarse/status\""));
        assert!(p.contains(&format!("\"identifiers\":[\"maarse_{}\"]", "12".repeat(16))));
        assert!(!d[2].1.contains("unit_of_measurement"), "energy status has no unit");
        for (_, p) in &d {
            assert!(p.starts_with('{') && p.ends_with("}}"), "{p}");
            assert_eq!(p.matches('{').count(), p.matches('}').count(), "{p}");
        }
    }

    /// A small mock broker: accepts CONNECT, answers CONNACK and collects
    /// PUBLISH packets.
    #[test]
    fn publishes_to_broker() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap().to_string();
        let broker = std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut buf = Vec::new();
            let mut tmp = [0u8; 4096];
            // CONNECT
            let n = s.read(&mut tmp).unwrap();
            buf.extend_from_slice(&tmp[..n]);
            assert_eq!(buf[0], 0x10);
            let (len, k) = read_remaining(&buf[1..]).unwrap();
            let connect = buf.drain(..1 + k + len).collect::<Vec<u8>>();
            s.write_all(&[0x20, 2, 0, 0]).unwrap();
            let mut got = Vec::new();
            while got.len() < 4 {
                while let Some((t, p, r, used)) = parse_publish(&buf) {
                    got.push((t, String::from_utf8(p).unwrap(), r));
                    buf.drain(..used);
                }
                if got.len() >= 4 {
                    break;
                }
                let n = s.read(&mut tmp).unwrap();
                assert!(n > 0);
                buf.extend_from_slice(&tmp[..n]);
            }
            (connect, got)
        });
        let mut m = Mqtt::spawn(cfg(addr), "maarse-test".into());
        let a = Address([7; 16]);
        m.telemetry(&a, 1, &Telemetry { battery_mv: 3700, energy: 2, ..Default::default() });
        let (connect, got) = broker.join().unwrap();
        assert!(connect.windows(7).any(|w| w == b"offline"));
        assert_eq!(got[0], ("maarse/status".into(), "online".into(), true));
        assert!(got[1].0.starts_with("homeassistant/sensor/maarse_07070707/"));
        assert!(got.iter().all(|g| g.2), "everything is retained");
        drop(m);
    }
}
