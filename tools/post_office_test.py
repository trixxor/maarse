# Hardware test of the post office through maarse-server's LoRa modem.
#
#   radio user (XIAO, own firmware)  <-LoRa->  modem (Heltec, modem mode)
#                                                  |  USB
#                                            maarse-server (test instance)
#                                                  |  TCP backbone
#                                            maarse-host "internet user" (no radio)
#
# 1. online: messages both ways with receipts;
# 2. offline: the XIAO is moved to another frequency (cannot be reached);
#    the internet user writes; the mail ends in the post offices;
# 3. back: the XIAO returns to its frequency and fetches its mail over LoRa.
# The XIAO's radio settings are always restored.
#
#   python tools/post_office_test.py [--modem COM6] [--radio COM7] [--bin target/release]
import argparse, json, os, struct, subprocess, sys, tempfile, time, http.cookiejar, urllib.request, urllib.parse
import serial, websocket

ap = argparse.ArgumentParser()
ap.add_argument("--modem", default="COM6")
ap.add_argument("--radio", default="COM7")
ap.add_argument("--bin", default="target/release")
ap.add_argument("--wait", type=int, default=600, help="seconds to wait for the mail after the radio is back")
ap.add_argument("--server", help="use a running server (host:port of its backbone) instead of starting a test server; its modem must be the other radio")
ap.add_argument("--dashboard", default="http://127.0.0.1:8430", help="dashboard of the server (for its telemetry)")
ap.add_argument("--password", default="post-office-test-1", help="admin password of the dashboard (empty = do not read telemetry)")
a = ap.parse_args()
EXE = ".exe" if os.name == "nt" else ""
SERVER = os.path.abspath(os.path.join(a.bin, "maarse-server" + EXE))
HOST = os.path.abspath(os.path.join(a.bin, "maarse-host" + EXE))
TMP = tempfile.mkdtemp(prefix="maarse-post-test-")
T0 = time.time()
procs = []


def say(msg):
    print(f"[{time.time() - T0:6.0f} s] {msg}", flush=True)


def frame(kind, *parts):
    body = bytes([kind]) + b"".join(parts)
    return struct.pack("<H", len(body)) + body


def frames(buf):
    out = []
    while len(buf) >= 2:
        n = struct.unpack("<H", buf[:2])[0]
        if len(buf) < 2 + n:
            break
        out.append(buf[2:2 + n])
        buf = buf[2 + n:]
    return out, buf


class Radio:
    """The XIAO over USB (companion protocol)."""

    def __init__(self, port):
        self.s = serial.Serial(port, 115200, timeout=0.2)
        time.sleep(0.5)
        self.s.read(65536)
        self.buf, self.events = b"", []

    def send(self, b):
        self.s.write(b)

    def pump(self, secs=0.5):
        end = time.time() + secs
        while time.time() < end:
            self.buf += self.s.read(4096)
            fs, self.buf = frames(self.buf)
            self.events += fs

    def wait(self, pred, secs):
        end = time.time() + secs
        while time.time() < end:
            for i, f in enumerate(self.events):
                if pred(f):
                    del self.events[i]
                    return f
            self.pump(0.5)
        return None


class Web:
    """The internet user's host over WebSocket."""

    def __init__(self, port):
        self.ws = websocket.create_connection(f"ws://127.0.0.1:{port}/ws", origin=f"http://127.0.0.1:{port}", timeout=5)
        self.buf, self.events = b"", []

    def send(self, b):
        self.ws.send_binary(b)

    def pump(self, secs=0.5):
        end = time.time() + secs
        self.ws.settimeout(0.2)
        while time.time() < end:
            try:
                self.buf += self.ws.recv()
            except websocket.WebSocketTimeoutException:
                continue
            fs, self.buf = frames(self.buf)
            self.events += fs

    def wait(self, pred, secs):
        end = time.time() + secs
        while time.time() < end:
            for i, f in enumerate(self.events):
                if pred(f):
                    del self.events[i]
                    return f
            self.pump(0.5)
        return None


def telemetry():
    cj = http.cookiejar.CookieJar()
    op = urllib.request.build_opener(urllib.request.HTTPCookieProcessor(cj))
    op.open(urllib.request.Request(a.dashboard + "/login", data=urllib.parse.urlencode({"password": a.password}).encode()))
    t = json.loads(op.open(a.dashboard + "/api/telemetry").read())
    s = json.loads(op.open(a.dashboard + "/api/debug/snapshot").read())
    return t, s


radio = None
saved_radio = None
ok = {"online A->X": False, "online X->A": False, "stored": False, "fetched": 0}
try:
    # ---------- start ----------
    server_ep = a.server or "127.0.0.1:4330"
    if not a.server:
        sd = os.path.join(TMP, "server")
        subprocess.run([SERVER, "--set-admin-password", "post-office-test-1", "--data", sd], check=True, capture_output=True)
        procs.append(subprocess.Popen([SERVER, "--data", sd, "--listen", "127.0.0.1:4330", "--public-addr", "127.0.0.1:4330", "--http", "127.0.0.1:8430", "--serial", a.modem],
                                      stdout=open(os.path.join(TMP, "server.log"), "w"), stderr=subprocess.STDOUT))
        time.sleep(2)
    procs.append(subprocess.Popen([HOST, "--data", os.path.join(TMP, "host"), "--no-browser", "--listen", "off", "--http", "127.0.0.1:8431", "--peer", server_ep],
                                  stdout=open(os.path.join(TMP, "host.log"), "w"), stderr=subprocess.STDOUT))
    time.sleep(2)
    web = Web(8431)
    web.send(frame(0x24, b"Internet user"))
    web.send(frame(0x01))
    A = web.wait(lambda f: f[0] == 0x81, 10)[1:17]
    radio = Radio(a.radio)
    radio.send(frame(0x01))
    X = radio.wait(lambda f: f[0] == 0x81, 10)[1:17]
    radio.send(frame(0x0b))
    saved_radio = radio.wait(lambda f: f[0] == 0x92, 10)[1:17]
    freq = struct.unpack("<I", saved_radio[:4])[0]
    say(f"internet user {A.hex()[:8]}, radio user {X.hex()[:8]} on {freq / 1e6:.3f} MHz; logs in {TMP}")

    # ---------- keys: announces both ways ----------
    for attempt in range(6):
        radio.send(frame(0x05))
        web.send(frame(0x05))
        got_a = web.wait(lambda f: f[0] == 0x87 and f[1:17] == X, 25)
        got_x = radio.wait(lambda f: f[0] == 0x87 and f[1:17] == A, 5)
        if got_a and got_x:
            break
        say(f"announces: host heard radio user {bool(got_a)}, radio user heard host {bool(got_x)} — again")
    say("both know each other's keys")

    # ---------- 1. online ----------
    web.send(frame(0x02, struct.pack("<H", 1), X, b"online: hello from the internet"))
    r = radio.wait(lambda f: f[0] == 0x83 and b"online: hello from the internet" in f, 90)
    ack = web.wait(lambda f: f[0] == 0x85, 60)
    ok["online A->X"] = bool(r and ack)
    say(f"1a internet -> radio: received {bool(r)}, receipt {bool(ack)}" + (f", {r[17]} hops" if r else ""))
    radio.send(frame(0x02, struct.pack("<H", 2), A, b"online: hello from the radio"))
    r = web.wait(lambda f: f[0] == 0x83 and b"online: hello from the radio" in f, 90)
    ack = radio.wait(lambda f: f[0] == 0x85, 60)
    ok["online X->A"] = bool(r and ack)
    say(f"1b radio -> internet: received {bool(r)}, receipt {bool(ack)}")

    # ---------- 2. offline ----------
    off = bytearray(saved_radio)
    off[0:4] = struct.pack("<I", 868_100_000)
    radio.send(frame(0x0a, bytes(off)))
    radio.wait(lambda f: f[0] == 0x92, 10)
    say("radio user moved to 868.100 MHz (unreachable)")
    for i in range(2):
        web.send(frame(0x02, struct.pack("<H", 10 + i), X, f"post office test {i + 1}: written while you were away".encode()))
        time.sleep(2)
    say("internet user wrote 2 messages; waiting for them to reach the post offices (no receipt in 45 s)")
    deadline = time.time() + 240
    if not a.password:
        say("2  (no dashboard password: waiting 75 s for the deposit instead of reading the server's post office)")
        web.pump(75)
        ok["stored"] = None
        deadline = 0
    while time.time() < deadline:
        web.pump(5)
        try:
            t, s = telemetry()
        except Exception:
            continue
        kept = sum(m["count"] for m in s.get("mailbox", []) if m["recipient"] == X.hex())
        if kept >= 2:
            ok["stored"] = True
            say(f"2  the server keeps {kept} message(s) for the radio user (post office ops: {t.get('mail_ops')})")
            break
    if ok["stored"] is False:
        say("2  the server did not get the mail within 4 minutes (see the logs)")

    # ---------- 3. back ----------
    radio.send(frame(0x0a, bytes(saved_radio)))
    radio.wait(lambda f: f[0] == 0x92, 10)
    say(f"radio user back on {freq / 1e6:.3f} MHz; waiting up to {a.wait} s for it to fetch its mail over LoRa")
    got = set()
    deadline = time.time() + a.wait
    while time.time() < deadline and len(got) < 2:
        f = radio.wait(lambda f: f[0] == 0x83 and b"post office test" in f, 10)
        if f:
            txt = f[18:].decode(errors="replace")
            got.add(txt)
            say(f"3  radio user received: '{txt}' ({f[17]} hops)")
    ok["fetched"] = len(got)
    if not a.password:
        raise SystemExit(0)
    t, s = telemetry()
    me = [n for n in t["nodes"] if n["addr"] == X.hex()]
    say(f"server census of the radio user: {json.dumps(me[0], ensure_ascii=True)[:300] if me else 'not heard'}")
    say(f"server mail ops: {t.get('mail_ops')}; delivery: seen {t['delivery']['data_seen']}, receipts {t['delivery']['data_acked']}")
    m = list(t["modems"].values())
    if m:
        d = dict(m[0]["last"])
        say(f"modem: rx ok {d.get(30)}, tx ok {d.get(32)}, airtime {d.get(40)} ms, duty {d.get(41)} permille, avg SNR {d.get(37)} dB, noise {d.get(80)} dBm")
finally:
    if radio and saved_radio:
        radio.send(frame(0x0a, bytes(saved_radio)))
        radio.wait(lambda f: f[0] == 0x92, 10)
        print("radio settings restored", flush=True)
    for p in procs:
        p.kill()
    print("RESULT:", json.dumps(ok), flush=True)
    print("logs in", TMP, flush=True)
