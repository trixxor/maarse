//! Test suites: catalogue, queue, running as subprocesses, parsing of output
//! and history.

use crate::{broadcast, broadcast_raw, cfg};
use serde_json::{Map, Value, json};
use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::channel;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy, PartialEq, Debug)]
enum Parser {
    Build,
    Cargo,
    Py,
    Radio,
    Sim,
    Pow,
}

#[derive(Clone)]
struct Step {
    label: String,
    prog: String,
    args: Vec<String>,
    parser: Parser,
}

#[derive(Clone)]
struct Suite {
    id: String,
    group: &'static str,
    title: String,
    desc: String,
    /// Static list of what the suite tests (shown before running).
    items: Vec<String>,
    steps: Vec<Step>,
    hardware: bool,
    runnable: bool,
}

fn step(label: &str, prog: &str, args: &[&str], parser: Parser) -> Step {
    Step {
        label: label.into(),
        prog: prog.into(),
        args: args.iter().map(|s| s.to_string()).collect(),
        parser,
    }
}

fn exe(name: &str) -> String {
    cfg()
        .root
        .join("target")
        .join("release")
        .join(format!("{name}{}", std::env::consts::EXE_SUFFIX))
        .to_string_lossy()
        .into_owned()
}

fn python() -> &'static str {
    if cfg!(windows) { "python" } else { "python3" }
}

fn members() -> Vec<String> {
    let t = std::fs::read_to_string(cfg().root.join("Cargo.toml")).unwrap_or_default();
    let Some(line) = t.lines().find(|l| l.trim_start().starts_with("members")) else {
        return vec![];
    };
    line.split('"')
        .skip(1)
        .step_by(2)
        .map(String::from)
        .collect()
}

fn crate_desc(c: &str) -> &'static str {
    match c {
        "maarse-core" => "The protocol core: packet format, router (routes, hop retransmission, acknowledgements), duplicate filter, duty cycle, time on air, proof of work (stamps), regions, the companion protocol and content formats.",
        "maarse-crypto" => "Cryptography: identities (Ed25519/X25519), signed announces, encrypted direct messages, channels and file chunks, safety numbers.",
        "maarse-host" => "The host program: mailbox, file transfer, backbone links, DHT, remote administration, web server and fuzzing of the host's decoders.",
        "maarse-sim" => "The simulator (compile check only; the scenarios have their own suites below).",
        "maarse-uf2" => "The UF2 tool that packs firmware for drag-and-drop flashing.",
        "maarse-lab" => "This test lab.",
        _ => "Crate in the workspace.",
    }
}

fn sim_scenarios() -> Vec<(&'static str, &'static str, &'static str, Vec<&'static str>)> {
    vec![
        (
            "sim-120",
            "120 nodes, 30×30 km",
            "The default scenario: 30 repeaters and 90 clients placed at random, 60 direct messages and 10 channel messages over 15 minutes.",
            vec!["--nodes", "120", "--repeaters", "30", "--area-km", "30"],
        ),
        (
            "sim-200",
            "200 nodes, 60×60 km",
            "Large, sparse net: many nodes are only poorly covered. Shows the weakness of long, fragile routes.",
            vec!["--nodes", "200", "--repeaters", "40", "--area-km", "60"],
        ),
        (
            "sim-highway",
            "Motorway 100 km, car at 110 km/h",
            "Repeaters on a line along a motorway; a car drives past and writes in turn with a fixed node at the end of the road. Tests routes to a node that moves.",
            vec!["--highway"],
        ),
    ]
}

fn catalog() -> Vec<Suite> {
    let mut v = Vec::new();
    for c in members() {
        if c == "maarse-fuzz" {
            continue;
        }
        v.push(Suite {
            id: format!("cargo:{c}"),
            group: "Unit tests (cargo test)",
            title: format!("cargo test -p {c}"),
            desc: crate_desc(&c).into(),
            items: vec![],
            steps: vec![step("cargo test", "cargo", &["test", "-p", &c], Parser::Cargo)],
            hardware: false,
            runnable: true,
        });
    }
    v.push(Suite {
        id: "fuzz".into(),
        group: "Fuzzing",
        title: "maarse-fuzz (release, 100 000 inputs per target)".into(),
        desc: "All decoders that read bytes from outside are fed half random inputs and half mutations of valid frames. The program must never crash, and decoded input must re-encode and decode unchanged.".into(),
        items: vec![
            "Packet::decode".into(),
            "companion Command::decode / Event::decode".into(),
            "FrameDecoder (stream cut into random pieces)".into(),
            "content::parse".into(),
            "Router::on_receive".into(),
            "RadioConfig".into(),
            "verify_announce, open_direct, open_channel, open_chunk".into(),
        ],
        steps: vec![step(
            "cargo test --release",
            "cargo",
            &["test", "-p", "maarse-fuzz", "--release"],
            Parser::Cargo,
        )],
        hardware: false,
        runnable: true,
    });
    let build_host = step(
        "build maarse-host",
        "cargo",
        &["build", "--release", "-p", "maarse-host"],
        Parser::Build,
    );
    v.push(Suite {
        id: "e2e".into(),
        group: "End to end",
        title: "tools/e2e_test.py".into(),
        desc: "Starts three real maarse-host processes (A, B and the mailbox P), connected over backbone links, and drives them over WebSocket like the web client does.".into(),
        items: [
            "messages both ways",
            "channel message",
            "private channel (random key) that B remembers after a restart",
            "20 KB file transferred and verified",
            "too large file rejected",
            "B offline: the message goes to the mailbox and is fetched when B comes back",
            "remote administration: the administrator gets access, others are rejected",
            "a foreign web site cannot connect",
            "regions (#trip@nordjylland)",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect(),
        steps: vec![
            build_host.clone(),
            Step {
                label: "e2e_test.py".into(),
                prog: python().into(),
                args: vec!["tools/e2e_test.py".into(), exe("maarse-host")],
                parser: Parser::Py,
            },
        ],
        hardware: false,
        runnable: true,
    });
    v.push(Suite {
        id: "dht".into(),
        group: "End to end",
        title: "tools/dht_test.py".into(),
        desc: "Three backbone hosts without packet connection to each other, only with B1 as DHT bootstrap. A writes to C, which A has never heard; the address is found via the DHT.".into(),
        items: vec![
            "A -> C found via DHT and delivered".into(),
            "acknowledgement back".into(),
            "reply C -> A".into(),
        ],
        steps: vec![
            build_host,
            Step {
                label: "dht_test.py".into(),
                prog: python().into(),
                args: vec!["tools/dht_test.py".into(), exe("maarse-host")],
                parser: Parser::Py,
            },
        ],
        hardware: false,
        runnable: true,
    });
    let ports = &cfg().ports;
    let mut radio_args = vec!["tools/radio_test.py".to_string()];
    radio_args.extend(ports.iter().take(2).cloned());
    v.push(Suite {
        id: "radio".into(),
        group: "Hardware",
        title: "tools/radio_test.py".into(),
        desc: format!(
            "Needs boards. Two real radios over USB/serial: info, announce over LoRa, direct message both ways with acknowledgement, status. {}",
            if ports.len() >= 2 {
                format!("Ports: {} and {}.", ports[0], ports[1])
            } else {
                "Start maarse-lab with --port COM6 --port COM7 to be able to run it.".into()
            }
        ),
        items: vec![
            "Info and radio from both nodes".into(),
            "Announce over LoRa".into(),
            "Direct message A -> B (received + acknowledged)".into(),
            "Direct message B -> A (received + acknowledged)".into(),
        ],
        steps: vec![Step {
            label: "radio_test.py".into(),
            prog: python().into(),
            args: radio_args,
            parser: Parser::Radio,
        }],
        hardware: true,
        runnable: ports.len() >= 2,
    });
    let build_sim = step(
        "build maarse-sim",
        "cargo",
        &["build", "--release", "-p", "maarse-sim"],
        Parser::Build,
    );
    for (id, title, desc, args) in sim_scenarios() {
        let mut steps = vec![build_sim.clone()];
        for (mode, extra) in [("Må:rse", vec![]), ("flood, 7 hops", vec!["--mode", "flood", "--hop-limit", "7"])] {
            let mut a: Vec<String> = args.iter().map(|s| s.to_string()).collect();
            a.extend(extra.iter().map(|s| s.to_string()));
            steps.push(Step {
                label: mode.into(),
                prog: exe("maarse-sim"),
                args: a,
                parser: Parser::Sim,
            });
        }
        v.push(Suite {
            id: id.into(),
            group: "Simulator",
            title: title.into(),
            desc: format!("{desc} Runs both Må:rse and flood (Meshtastic-like) for comparison."),
            items: vec!["Må:rse".into(), "flood, 7 hops".into()],
            steps,
            hardware: false,
            runnable: true,
        });
    }
    v.push(Suite {
        id: "pow".into(),
        group: "Performance",
        title: "Proof of work (stamp benchmark)".into(),
        desc: "Measures how many SHA-256 attempts per second the stamp search manages (release), and what that means for the time to stamp an announce and a path request on this machine.".into(),
        items: vec!["attempts/s, announce time, path request time".into()],
        steps: vec![step(
            "stamp_benchmark",
            "cargo",
            &["test", "-p", "maarse-core", "--release", "--lib", "stamp_benchmark", "--", "--ignored", "--nocapture"],
            Parser::Pow,
        )],
        hardware: false,
        runnable: true,
    });
    v
}

// ---------- state ----------

struct State {
    suites: Vec<Suite>,
    queue: VecDeque<String>,
    running: Option<String>,
    child: Option<Arc<Mutex<Child>>>,
    history: Vec<Value>,
    tests: Value,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);
static WAKE: Condvar = Condvar::new();
static STOP: AtomicBool = AtomicBool::new(false);

fn with<R>(f: impl FnOnce(&mut State) -> R) -> R {
    f(STATE.lock().unwrap().as_mut().unwrap())
}

fn hist_path() -> PathBuf {
    cfg().data.join("history.json")
}
fn tests_path() -> PathBuf {
    cfg().data.join("tests.json")
}

pub fn init() {
    let history = std::fs::read_to_string(hist_path())
        .ok()
        .and_then(|s| serde_json::from_str::<Vec<Value>>(&s).ok())
        .unwrap_or_default();
    let tests = std::fs::read_to_string(tests_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(Value::Null);
    let have_tests = !tests.is_null();
    *STATE.lock().unwrap() = Some(State {
        suites: catalog(),
        queue: VecDeque::new(),
        running: None,
        child: None,
        history,
        tests,
    });
    thread::spawn(worker);
    if !have_tests {
        discover();
    }
}

pub fn hello() -> Value {
    with(|s| {
        let suites: Vec<Value> = s
            .suites
            .iter()
            .map(|x| {
                json!({"id":x.id,"group":x.group,"title":x.title,"desc":x.desc,"items":x.items,
                       "hardware":x.hardware,"runnable":x.runnable,
                       "steps":x.steps.iter().map(|st| st.label.clone()).collect::<Vec<_>>()})
            })
            .collect();
        json!({"type":"hello","suites":suites,"tests":s.tests,"history":s.history,
               "queue":s.queue,"running":s.running,"ports":cfg().ports,
               "root":cfg().root.to_string_lossy()})
    })
}

fn send_queue() {
    let v = with(|s| json!({"type":"queue","queue":s.queue,"running":s.running}));
    broadcast(&v);
}

pub fn enqueue(id: &str) {
    let ok = with(|s| {
        let Some(su) = s.suites.iter().find(|x| x.id == id) else {
            return false;
        };
        if !su.runnable || s.queue.iter().any(|q| q == id) || s.running.as_deref() == Some(id) {
            return false;
        }
        s.queue.push_back(id.to_string());
        true
    });
    if ok {
        WAKE.notify_all();
        send_queue();
    }
}

pub fn enqueue_all() {
    let ids: Vec<String> = with(|s| {
        s.suites
            .iter()
            .filter(|x| x.runnable && !x.hardware)
            .map(|x| x.id.clone())
            .collect()
    });
    for id in ids {
        enqueue(&id);
    }
}

pub fn stop() {
    STOP.store(true, Ordering::SeqCst);
    let c = with(|s| {
        s.queue.clear();
        s.child.clone()
    });
    if let Some(c) = c {
        let _ = c.lock().unwrap().kill();
    }
    send_queue();
}

pub fn clear_history() {
    with(|s| s.history.clear());
    let _ = std::fs::write(hist_path(), "[]");
    broadcast(&json!({"type":"history","history":[]}));
}

fn now_s() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn worker() {
    loop {
        let id = {
            let mut g = STATE.lock().unwrap();
            loop {
                let s = g.as_mut().unwrap();
                if let Some(id) = s.queue.pop_front() {
                    s.running = Some(id.clone());
                    break id;
                }
                g = WAKE.wait(g).unwrap();
            }
        };
        STOP.store(false, Ordering::SeqCst);
        send_queue();
        let suite = with(|s| s.suites.iter().find(|x| x.id == id).cloned());
        if let Some(suite) = suite {
            if suite.hardware {
                crate::hw::pause(true);
            }
            run_suite(&suite);
            if suite.hardware {
                crate::hw::pause(false);
            }
        }
        with(|s| {
            s.running = None;
            s.child = None;
        });
        send_queue();
    }
}

/// State of one suite's run; filled in by the output parsers.
#[derive(Default)]
struct Run {
    passed: u32,
    failed: u32,
    ignored: u32,
    metrics: Map<String, Value>,
    group: String,
    last_row: Option<Instant>,
    step_start: Option<Instant>,
    sim: Map<String, Value>,
    last_err: String,
}

fn row(suite: &str, run: &mut Run, name: &str, status: &str, ms: u128) {
    match status {
        "ok" => run.passed += 1,
        "fail" => run.failed += 1,
        _ => run.ignored += 1,
    }
    broadcast(&json!({"type":"row","suite":suite,"group":run.group,"name":name,"status":status,"ms":ms}));
}

fn run_suite(suite: &Suite) {
    let t0 = Instant::now();
    broadcast(&json!({"type":"start","suite":suite.id}));
    let mut run = Run::default();
    let mut ok = true;
    for st in &suite.steps {
        if STOP.load(Ordering::SeqCst) {
            ok = false;
            break;
        }
        broadcast(&json!({"type":"line","suite":suite.id,"stream":"info","text":format!("$ {} {}", st.prog, st.args.join(" "))}));
        run.step_start = Some(Instant::now());
        run.last_row = run.step_start;
        run.group = st.label.clone();
        run.sim.clear();
        let code = run_step(&suite.id, st, &mut run);
        let step_ok = code == Some(0);
        match st.parser {
            Parser::Sim => {
                let mut m = std::mem::take(&mut run.sim);
                let name = format!(
                    "{}: delivered {} %, acked {} %, median {} s, airtime {} s",
                    st.label,
                    m.get("delivered").map_or("?".into(), |v| v.to_string()),
                    m.get("acked").map_or("?".into(), |v| v.to_string()),
                    m.get("median_s").map_or("?".into(), |v| v.to_string()),
                    m.get("airtime_s").map_or("?".into(), |v| v.to_string()),
                );
                let ms = run.step_start.unwrap().elapsed().as_millis();
                row(&suite.id, &mut run, &name, if step_ok { "ok" } else { "fail" }, ms);
                m.insert("mode".into(), json!(st.label));
                run.metrics.insert(st.label.clone(), Value::Object(m));
            }
            _ if !step_ok => {
                let why = match code {
                    None => "aborted".to_string(),
                    Some(c) => format!("exit code {c}"),
                };
                let ms = run.step_start.unwrap().elapsed().as_millis();
                let txt = if run.last_err.is_empty() {
                    format!("{} {why}", st.label)
                } else {
                    format!("{} {why}: {}", st.label, run.last_err)
                };
                if run.failed == 0 || st.parser == Parser::Build {
                    row(&suite.id, &mut run, &txt, "fail", ms);
                }
            }
            _ => {}
        }
        if !step_ok {
            ok = false;
            break;
        }
    }
    let ms = t0.elapsed().as_millis() as u64;
    let rec = json!({"suite":suite.id,"title":suite.title,"at":now_s(),"ms":ms,"ok":ok && run.failed == 0,
                     "passed":run.passed,"failed":run.failed,"ignored":run.ignored,"metrics":run.metrics});
    broadcast(&json!({"type":"done","rec":rec}));
    with(|s| {
        s.history.push(rec);
        let n = s.history.len();
        if n > 300 {
            s.history.drain(..n - 300);
        }
        let _ = std::fs::write(hist_path(), Value::Array(s.history.clone()).to_string());
    });
}

/// Run one step and stream the output. Returns the exit code (None = aborted).
fn run_step(suite: &str, st: &Step, run: &mut Run) -> Option<i32> {
    let mut cmd = Command::new(&st.prog);
    cmd.args(&st.args)
        .current_dir(&cfg().root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("CARGO_TERM_COLOR", "never")
        .env("PYTHONUNBUFFERED", "1")
        .env("PYTHONIOENCODING", "utf-8");
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            run.last_err = format!("could not start {}: {e}", st.prog);
            broadcast(&json!({"type":"line","suite":suite,"stream":"err","text":run.last_err}));
            return Some(-1);
        }
    };
    let (tx, rx) = channel::<(bool, String)>();
    for (is_err, r) in [
        (false, child.stdout.take().map(|x| Box::new(x) as Box<dyn std::io::Read + Send>)),
        (true, child.stderr.take().map(|x| Box::new(x) as Box<dyn std::io::Read + Send>)),
    ] {
        let tx = tx.clone();
        if let Some(r) = r {
            thread::spawn(move || {
                for line in BufReader::new(r).split(b'\n').map_while(Result::ok) {
                    let l = String::from_utf8_lossy(&line).trim_end().to_string();
                    if tx.send((is_err, l)).is_err() {
                        break;
                    }
                }
            });
        }
    }
    drop(tx);
    let child = Arc::new(Mutex::new(child));
    with(|s| s.child = Some(child.clone()));
    loop {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok((is_err, l)) => {
                broadcast(&json!({"type":"line","suite":suite,"stream":if is_err {"err"} else {"out"},"text":l}));
                parse(suite, st.parser, &l, run);
                if is_err && !l.trim().is_empty() {
                    run.last_err = l.trim().chars().take(200).collect();
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(_) => break,
        }
    }
    let status = child.lock().unwrap().wait().ok()?;
    if STOP.load(Ordering::SeqCst) {
        return None;
    }
    status.code()
}

fn num(s: &str) -> Option<f64> {
    let t: String = s
        .trim()
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.' || *c == '-')
        .collect();
    t.parse().ok()
}

fn after<'a>(l: &'a str, key: &str) -> Option<&'a str> {
    l.find(key).map(|i| &l[i + key.len()..])
}

fn parse(suite: &str, p: Parser, l: &str, run: &mut Run) {
    let since = |run: &mut Run| {
        let now = Instant::now();
        let ms = run.last_row.map_or(0, |t| now.duration_since(t).as_millis());
        run.last_row = Some(now);
        ms
    };
    match p {
        Parser::Build => {}
        Parser::Cargo | Parser::Pow => {
            let t = l.trim();
            if let Some(rest) = t.strip_prefix("Running ") {
                // "Running unittests src\lib.rs (target\debug\deps\maarse_core-abc.exe)"
                let src = rest.split(" (").next().unwrap_or(rest);
                run.group = src.trim_start_matches("unittests ").replace('\\', "/");
                run.step_start = Some(Instant::now());
            } else if let Some(rest) = t.strip_prefix("Doc-tests ") {
                run.group = format!("doc-tests {rest}");
                run.step_start = Some(Instant::now());
            } else if let Some(rest) = t.strip_prefix("test ")
                && let Some((name, res)) = rest.rsplit_once(" ... ")
            {
                let status = if res.starts_with("ok") {
                    "ok"
                } else if res.starts_with("FAILED") {
                    "fail"
                } else if res.starts_with("ignored") {
                    "ignored"
                } else {
                    return;
                };
                let ms = run.step_start.map_or(0, |t| t.elapsed().as_millis());
                row(suite, run, name, status, ms);
            }
            if p == Parser::Pow && t.contains("attempts/s") {
                // "123 attempts/s; announce (18 bits) ≈ 2.10 ms, path request (12 bits) ≈ 0.033 ms"
                let rate = num(t).unwrap_or(0.0);
                run.metrics.insert("attempts_per_s".into(), json!(rate));
                let mut parts = t.split('≈').skip(1);
                if let Some(a) = parts.next().and_then(num) {
                    run.metrics.insert("announce_ms".into(), json!(a));
                }
                if let Some(b) = parts.next().and_then(num) {
                    run.metrics.insert("path_request_ms".into(), json!(b));
                }
                run.group = "benchmark".into();
                let ms = since(run);
                row(suite, run, t, "ok", ms);
            }
        }
        Parser::Py => {
            let t = l.trim();
            if let Some(m) = t.strip_prefix("OK ") {
                let ms = since(run);
                row(suite, run, m.trim(), "ok", ms);
            } else if t.starts_with("FAIL") || t.contains("AssertionError") {
                let ms = since(run);
                row(suite, run, t, "fail", ms);
            }
        }
        Parser::Radio => {
            let t = l.trim();
            if let Some(g) = t.strip_prefix("== ") {
                run.group = g.trim_end_matches(" ==").to_string();
            } else if let Some(r) = after(t, "-> received:") {
                let ok = r.matches("YES").count() == 2;
                let ms = since(run);
                row(suite, run, &format!("received:{r}"), if ok { "ok" } else { "fail" }, ms);
            } else if let Some(r) = t.strip_prefix("RESULT:") {
                let ms = since(run);
                let ok = r.trim() == "OK";
                row(suite, run, &format!("result {}", r.trim()), if ok { "ok" } else { "fail" }, ms);
            } else if t.starts_with("FAIL") {
                let ms = since(run);
                row(suite, run, t, "fail", ms);
            }
        }
        Parser::Sim => sim_metrics(l, &mut run.sim),
    }
}

/// Read key figures from the simulator's report.
fn sim_metrics(l: &str, m: &mut Map<String, Value>) {
    let t = l.trim();
    let mut put = |k: &str, v: Option<f64>| {
        if let Some(v) = v {
            m.insert(k.into(), json!(v));
        }
    };
    if let Some(r) = t.strip_prefix("Delivered:") {
        put("delivered", num(r));
    } else if let Some(r) = t.strip_prefix("Acknowledged:") {
        put("acked", num(r));
    } else if let Some(r) = after(t, "Latency:") {
        put("median_s", after(r, "median").and_then(num));
        put("p90_s", after(r, "90 % below").and_then(num));
    } else if let Some(r) = t.strip_prefix("Total airtime:") {
        put("airtime_s", num(r));
    } else if let Some(r) = t.strip_prefix("Transmissions:") {
        put("transmissions", num(r));
    } else if let Some(r) = after(t, "Lost to collisions (per receiver):") {
        put("collisions", num(r));
    } else if let Some(r) = after(t, "Reached on avg") {
        put("channel_pct", num(r));
    } else if let Some(r) = after(t, "Dropped due to duty cycle:") {
        put("duty_drop", num(r));
    }
}

// ---------- test list ----------

static DISCOVERING: AtomicBool = AtomicBool::new(false);

/// `cargo test --workspace -- --list`, grouped per crate and test target.
pub fn discover() {
    if DISCOVERING.swap(true, Ordering::SeqCst) {
        return;
    }
    thread::spawn(|| {
        broadcast(&json!({"type":"discovering","on":true}));
        let out = Command::new("cargo")
            .args(["test", "--workspace", "--", "--list"])
            .current_dir(&cfg().root)
            .env("CARGO_TERM_COLOR", "never")
            .stdin(Stdio::null())
            .output();
        DISCOVERING.store(false, Ordering::SeqCst);
        let Ok(out) = out else {
            broadcast(&json!({"type":"discovering","on":false}));
            return;
        };
        // stderr and stdout are separate; combine via the order of "Running".
        // With --format terse the names come on stdout in the same order
        // as the "Running" lines on stderr; we use empty lines as separators.
        let err = String::from_utf8_lossy(&out.stderr);
        let mut prev = String::new();
        let targets: Vec<(String, String)> = err
            .lines()
            .filter_map(|l| {
                let t = l.trim();
                if let Some(rest) = t.strip_prefix("Running ") {
                    let src = rest.split(" (").next().unwrap_or(rest);
                    let src = src.trim_start_matches("unittests ").replace('\\', "/");
                    let bin = rest.rsplit(['/', '\\']).next().unwrap_or("");
                    let mut krate = bin.rsplit_once('-').map_or(bin, |x| x.0).replace('_', "-");
                    // Integration tests (tests/*.rs) belong to the preceding crate.
                    if src.starts_with("tests/") && !prev.is_empty() {
                        krate = prev.clone();
                    }
                    prev = krate.clone();
                    Some((krate, src))
                } else {
                    t.strip_prefix("Doc-tests ")
                        .map(|c| (c.replace('_', "-"), "doc-tests".to_string()))
                }
            })
            .collect();
        let so = String::from_utf8_lossy(&out.stdout);
        // Every test binary writes its names followed by "N tests, M benchmarks".
        let mut blocks: Vec<Vec<String>> = vec![vec![]];
        for l in so.lines() {
            if let Some(n) = l.strip_suffix(": test") {
                blocks.last_mut().unwrap().push(n.to_string());
            } else if l.contains(" tests, ") || l.ends_with(" benchmarks") {
                blocks.push(vec![]);
            }
        }
        let mut crates: HashMap<String, Vec<Value>> = HashMap::new();
        for (i, (krate, src)) in targets.iter().enumerate() {
            let names = blocks.get(i).cloned().unwrap_or_default();
            crates
                .entry(krate.clone())
                .or_default()
                .push(json!({"target":src,"tests":names}));
        }
        let tests = json!(crates);
        let _ = std::fs::write(tests_path(), tests.to_string());
        with(|s| s.tests = tests.clone());
        broadcast(&json!({"type":"tests","tests":tests}));
        broadcast(&json!({"type":"discovering","on":false}));
    });
}

// ---------- live simulator ----------

static SIMS: Mutex<Vec<Arc<Mutex<Child>>>> = Mutex::new(Vec::new());

const SIM_FLAGS: &[&str] = &[
    "--nodes", "--repeaters", "--area-km", "--messages", "--channel-messages", "--seed", "--sf",
    "--duty", "--mode", "--hop-limit", "--duration", "--road-km", "--car-kmh",
];

pub fn sim_stop() {
    for c in SIMS.lock().unwrap().drain(..) {
        let _ = c.lock().unwrap().kill();
    }
}

/// Start the simulator with an event trace and pass the events on.
pub fn sim_live(tag: &str, args: Vec<String>) {
    let tag: String = tag.chars().filter(|c| c.is_ascii_alphanumeric()).take(8).collect();
    // Only known flags (the page cannot start anything but the simulator).
    let mut clean = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if a == "--highway" {
            clean.push(a.clone());
        } else if SIM_FLAGS.contains(&a.as_str()) && i + 1 < args.len() {
            let v = &args[i + 1];
            if v.chars().all(|c| c.is_ascii_alphanumeric() || c == '.') {
                clean.push(a.clone());
                clean.push(v.clone());
            }
            i += 1;
        }
        i += 1;
    }
    thread::spawn(move || {
        let bin = exe("maarse-sim");
        let send_text = |t: &str| broadcast(&json!({"type":"simtext","tag":tag,"text":t}));
        if !std::path::Path::new(&bin).exists() || stale_sim(&bin) {
            send_text("Building maarse-sim (release) …");
            let st = Command::new("cargo")
                .args(["build", "--release", "-p", "maarse-sim"])
                .current_dir(&cfg().root)
                .stdin(Stdio::null())
                .status();
            if !st.map(|s| s.success()).unwrap_or(false) {
                send_text("Building maarse-sim failed.");
                return;
            }
        }
        clean.push("--trace-json".into());
        clean.push("-".into());
        send_text(&format!("$ maarse-sim {}", clean.join(" ")));
        let child = Command::new(&bin)
            .args(&clean)
            .current_dir(&cfg().root)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn();
        let Ok(mut child) = child else {
            send_text("Could not start maarse-sim.");
            return;
        };
        let out = child.stdout.take().unwrap();
        let child = Arc::new(Mutex::new(child));
        SIMS.lock().unwrap().push(child.clone());
        let mut batch: Vec<String> = Vec::new();
        let mut last = Instant::now();
        let mut report = Map::new();
        let flush = |batch: &mut Vec<String>| {
            if !batch.is_empty() {
                broadcast_raw(format!(
                    "{{\"type\":\"simevs\",\"tag\":\"{tag}\",\"es\":[{}]}}",
                    batch.join(",")
                ));
                batch.clear();
            }
        };
        for line in BufReader::new(out).split(b'\n').map_while(Result::ok) {
            let l = String::from_utf8_lossy(&line).trim_end().to_string();
            if l.starts_with('{') {
                batch.push(l);
                if batch.len() >= 400 || last.elapsed() > Duration::from_millis(100) {
                    flush(&mut batch);
                    last = Instant::now();
                }
            } else {
                flush(&mut batch);
                sim_metrics(&l, &mut report);
                send_text(&l);
            }
        }
        flush(&mut batch);
        let _ = child.lock().unwrap().wait();
        broadcast(&json!({"type":"simdone","tag":tag,"report":report}));
    });
}

/// Is the binary older than the source code? (so the trace matches the code)
fn stale_sim(bin: &str) -> bool {
    let m = |p: PathBuf| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    let Some(b) = m(PathBuf::from(bin)) else {
        return true;
    };
    let src = cfg().root.join("maarse-sim").join("src");
    std::fs::read_dir(src)
        .map(|d| d.flatten().any(|e| m(e.path()).is_some_and(|t| t > b)))
        .unwrap_or(false)
}
