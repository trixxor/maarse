# End-to-end test of maarse-server: two hosts behind "NAT" (no listening
# port, no public address) that only connect out to one backbone node can
# write to each other through it, and a second backbone finds the first
# again after a restart from its saved contacts.
#   pip install websocket-client
#   python tools/backbone_test.py [target/release]
import os, struct, subprocess, sys, tempfile, time
import websocket

BIN = sys.argv[1] if len(sys.argv) > 1 else "target/release"
EXE = ".exe" if os.name == "nt" else ""
HOST = os.path.abspath(os.path.join(BIN, "maarse-host" + EXE))
BACKBONE = os.path.abspath(os.path.join(BIN, "maarse-server" + EXE))
TMP = tempfile.mkdtemp(prefix="maarse-server-test-")
procs = {}


def start(name, exe, args):
    d = os.path.join(TMP, name)
    os.makedirs(d, exist_ok=True)
    procs[name] = subprocess.Popen([exe, "--data", d] + args, stdout=open(os.path.join(d, "log.txt"), "a"), stderr=subprocess.STDOUT)
    time.sleep(0.8)


def stop(name):
    procs[name].terminate()
    procs[name].wait()


def frame(kind, *parts):
    body = bytes([kind]) + b"".join(parts)
    return struct.pack("<H", len(body)) + body


class Client:
    def __init__(self, port):
        self.ws = websocket.create_connection(f"ws://127.0.0.1:{port}/ws", origin=f"http://127.0.0.1:{port}", timeout=5)
        self.buf, self.events = b"", []

    def send(self, b):
        self.ws.send_binary(b)

    def pump(self, secs):
        end = time.time() + secs
        self.ws.settimeout(0.1)
        while time.time() < end:
            try:
                self.buf += self.ws.recv()
            except websocket.WebSocketTimeoutException:
                continue
            while len(self.buf) >= 2:
                n = struct.unpack("<H", self.buf[:2])[0]
                if len(self.buf) < 2 + n:
                    break
                self.events.append(self.buf[2:2 + n])
                self.buf = self.buf[2 + n:]

    def wait(self, pred, secs=20):
        end = time.time() + secs
        while time.time() < end:
            for f in self.events:
                if pred(f):
                    return f
            self.pump(0.3)
        raise AssertionError("waited in vain")


def log_has(name, text, secs=30):
    end = time.time() + secs
    while time.time() < end:
        with open(os.path.join(TMP, name, "log.txt"), encoding="utf-8", errors="replace") as f:
            if text in f.read():
                return True
        time.sleep(0.3)
    return False


try:
    # The backbone refuses to start without a public address.
    r = subprocess.run([BACKBONE, "--data", os.path.join(TMP, "nopub")], capture_output=True, text=True, timeout=10)
    assert r.returncode != 0 and "--public-addr" in (r.stderr + r.stdout), r.stderr
    print("  OK  maarse-server asks for --public-addr")

    start("bb1", BACKBONE, ["--listen", "127.0.0.1:4261", "--public-addr", "127.0.0.1:4261", "--http", "127.0.0.1:8195"])
    # The dashboard needs the admin login; the first password is in admin-password.txt.
    import urllib.request, urllib.error, http.cookiejar
    pw = open(os.path.join(TMP, "bb1", "admin-password.txt")).readline().strip()
    try:
        urllib.request.urlopen("http://127.0.0.1:8195/api/telemetry")
        raise AssertionError("telemetry without login")
    except urllib.error.HTTPError as e:
        assert e.code == 401, e.code
    jar = http.cookiejar.CookieJar()
    op = urllib.request.build_opener(urllib.request.HTTPCookieProcessor(jar))
    op.open(urllib.request.Request("http://127.0.0.1:8195/login", data=("password=" + pw).encode()))
    assert any(c.name == "maarse_session" for c in jar), "no session after login"
    print("  OK  dashboard behind the admin login")
    for name, http in (("a", 8191), ("b", 8192)):
        start(name, HOST, ["--no-browser", "--listen", "off", "--http", f"127.0.0.1:{http}", "--peer", "127.0.0.1:4261"])
    a, b = Client(8191), Client(8192)
    a.send(frame(0x01)); b.send(frame(0x01))
    addr_a = a.wait(lambda f: f[0] == 0x81)[1:17]
    addr_b = b.wait(lambda f: f[0] == 0x81)[1:17]
    # Announces make the keys known (and the backbone publishes both).
    a.send(frame(0x05)); b.send(frame(0x05))
    a.wait(lambda f: f[0] == 0x87 and f[1:17] == addr_b, 30)
    print("  OK  host A hears host B's announce through the backbone")
    a.send(frame(0x02, struct.pack("<H", 1), addr_b, b"hello through the backbone"))
    b.wait(lambda f: f[0] == 0x83 and b"hello through the backbone" in f, 30)
    a.wait(lambda f: f[0] == 0x85, 30)
    print("  OK  A -> B through the backbone, delivered and acknowledged")
    b.send(frame(0x02, struct.pack("<H", 2), addr_a, b"and back"))
    a.wait(lambda f: f[0] == 0x83 and b"and back" in f, 30)
    print("  OK  B -> A")

    # A second backbone joins through the first and saves its contacts.
    start("bb2", BACKBONE, ["--listen", "127.0.0.1:4262", "--public-addr", "127.0.0.1:4262", "--peer", "127.0.0.1:4261", "--http", "127.0.0.1:8196"])
    time.sleep(45)
    stop("bb2")
    saved = open(os.path.join(TMP, "bb2", "dht-contacts.txt")).read() if os.path.exists(os.path.join(TMP, "bb2", "dht-contacts.txt")) else ""
    print(f"  ..  bb2 saved contacts: {saved.strip() or '(none yet)'}")
    # Restart bb2 without --peer: it must find bb1 from what it saved.
    assert "127.0.0.1:4261" in saved, "bb2 saved no contact for bb1"
    os.replace(os.path.join(TMP, "bb2", "log.txt"), os.path.join(TMP, "bb2", "log-first-run.txt"))
    start("bb2", BACKBONE, ["--listen", "127.0.0.1:4262", "--public-addr", "127.0.0.1:4262", "--min-peers", "2", "--http", "127.0.0.1:8196"])
    assert log_has("bb2", "127.0.0.1:4261", 90), "bb2 did not reconnect to bb1 from its saved contacts"
    print("  OK  bb2 finds bb1 again after a restart without seeds")
    import json
    t = json.loads(op.open("http://127.0.0.1:8195/api/telemetry").read())
    assert len(t["nodes"]) >= 2 and t["delivery"]["data_seen"] >= 2, (len(t["nodes"]), t["delivery"])
    print(f"  OK  telemetry: {len(t['nodes'])} nodes in the census, {t['delivery']['data_seen']} messages seen, {t['delivery']['data_acked']} receipts")
    print("RESULT: OK")
finally:
    for n in list(procs):
        try:
            stop(n)
        except Exception:
            pass
    print("logs in", TMP)
