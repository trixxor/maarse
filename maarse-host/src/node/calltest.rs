//! Call test service: a node with `<data>/calltest/` answers every call by
//! itself. With `music.bin` there it first plays that music for 30 s, then
//! echoes what the caller says back after one second until they hang up.
//! Without music it only echoes.
//!
//! `music.bin` is Opus (48 kHz mono, 20 ms frames, as in calls) stored as
//! u16 LE length | frame, repeated; `tools/make_call_test.py` makes it from
//! any audio file. The service sends three frames per packet (60 ms), like
//! the apps.

use super::{Node, Timer};
use maarse_core::Address;
use maarse_core::content;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

const RING: u8 = 1;
const ANSWER: u8 = 2;
const END: u8 = 3;
const AUDIO: u8 = 4;
const PACKET: Duration = Duration::from_millis(60);
/// Bytes of (length | frame) that fit one sealed call datagram: 212 payload
/// - 69 forward secrecy - 8 datagram mark - 8 call header. Opus is variable
/// rate, so three frames often did not fit and the packet was dropped.
pub(super) const AUDIO_BUDGET: usize = 119;
const MUSIC_FOR: Duration = Duration::from_secs(30);
const ECHO_DELAY: Duration = Duration::from_secs(1);
/// A call nobody hangs up ends by itself.
const MAX_CALL: Duration = Duration::from_secs(600);

struct Call {
    peer: Address,
    id: u32,
    started: Instant,
    seq: u16,
    pos: usize,
    echo: VecDeque<(Instant, Vec<u8>)>,
}

#[derive(Default)]
pub(super) struct CallTest {
    music: Option<Vec<Vec<u8>>>,
    enabled: bool,
    call: Option<Call>,
}

impl CallTest {
    pub(super) fn load(data_dir: &std::path::Path) -> Self {
        let dir = data_dir.join("calltest");
        if !dir.is_dir() {
            return Self::default();
        }
        let music = std::fs::read(dir.join("music.bin")).ok().map(|b| {
            let mut frames = Vec::new();
            let mut i = 0;
            while i + 2 <= b.len() {
                let n = u16::from_le_bytes([b[i], b[i + 1]]) as usize;
                if i + 2 + n > b.len() || n == 0 || n > 255 {
                    break;
                }
                frames.push(b[i + 2..i + 2 + n].to_vec());
                i += 2 + n;
            }
            frames
        });
        Self { music: music.filter(|m| m.len() >= 3), enabled: true, call: None }
    }

    pub(super) fn enabled(&self) -> bool {
        self.enabled
    }
}

fn packet(kind: u8, id: u32, extra: &[u8]) -> Vec<u8> {
    let mut p = vec![content::CALL, kind];
    p.extend_from_slice(&id.to_le_bytes());
    p.extend_from_slice(extra);
    p
}

impl Node {
    /// True when the call test took the call data.
    pub(super) fn call_test_on(&mut self, from: Address, data: &[u8]) -> bool {
        if !self.call_test.enabled() || data.len() < 5 {
            return false;
        }
        let kind = data[0];
        let id = u32::from_le_bytes([data[1], data[2], data[3], data[4]]);
        match kind {
            RING => {
                let busy = self.call_test.call.as_ref().is_some_and(|c| c.peer != from && c.started.elapsed() < MAX_CALL);
                if busy {
                    let _ = self.send_sealed(from, &packet(END, id, &[]), super::Pending::Control);
                    return true;
                }
                self.log(format!("call test: answering {}", super::short(&from)));
                self.call_test.call = Some(Call { peer: from, id, started: Instant::now(), seq: 0, pos: 0, echo: VecDeque::new() });
                let _ = self.send_sealed(from, &packet(ANSWER, id, &[]), super::Pending::Control);
                self.schedule(PACKET, Timer::CallTest);
            }
            END => {
                if self.call_test.call.as_ref().is_some_and(|c| c.peer == from) {
                    self.call_test.call = None;
                    self.log("call test: call ended".to_string());
                }
            }
            AUDIO => {
                if let Some(c) = self.call_test.call.as_mut().filter(|c| c.peer == from) {
                    // Keep the frames of the packet (after the u16 sequence).
                    if data.len() > 7 && c.echo.len() < 200 {
                        c.echo.push_back((Instant::now(), data[7..].to_vec()));
                    }
                }
            }
            _ => {}
        }
        true
    }

    /// Every 60 ms during a test call: the next music packet or echo.
    pub(super) fn call_test_tick(&mut self) {
        let Some(c) = self.call_test.call.as_mut() else { return };
        if c.started.elapsed() > MAX_CALL {
            let (peer, id) = (c.peer, c.id);
            self.call_test.call = None;
            let _ = self.send_sealed(peer, &packet(END, id, &[]), super::Pending::Control);
            return;
        }
        // 60 ms of music (three frames), in as many packets as it takes.
        let mut bodies: Vec<Vec<u8>> = Vec::new();
        match self.call_test.music.as_ref().filter(|_| c.started.elapsed() < MUSIC_FOR) {
            Some(m) => {
                let mut b = Vec::new();
                for _ in 0..3 {
                    let f = &m[c.pos % m.len()];
                    c.pos += 1;
                    if !b.is_empty() && b.len() + 1 + f.len() > AUDIO_BUDGET {
                        bodies.push(std::mem::take(&mut b));
                    }
                    b.push(f.len() as u8);
                    b.extend_from_slice(f);
                }
                bodies.push(b);
            }
            None => {
                while let Some((t, _)) = c.echo.front() {
                    if t.elapsed() < ECHO_DELAY {
                        break;
                    }
                    bodies.extend(c.echo.pop_front().map(|(_, b)| b));
                }
            }
        }
        let (peer, id, mut seq) = (c.peer, c.id, c.seq);
        c.seq = c.seq.wrapping_add(bodies.len() as u16);
        for b in bodies {
            let mut extra = seq.to_le_bytes().to_vec();
            seq = seq.wrapping_add(1);
            extra.extend_from_slice(&b);
            if let Err(e) = self.send_call_audio(peer, &packet(AUDIO, id, &extra)) {
                self.log(format!("call test: packet not sent: {e}"));
            }
        }
        self.schedule(PACKET, Timer::CallTest);
    }
}
