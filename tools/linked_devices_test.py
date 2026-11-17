# End-to-end test of linked devices (one person, several devices) with three
# maarse-host processes on this machine:
#   A = the person's PC (master device), B = the person's phone, C = a friend.
# B and C connect to A over TCP. Checked:
#   1. A makes a person (24 words), B waits, A links B; both see the link.
#   2. C writes to the PERSON's address; it arrives on one device and the
#      other gets a sibling copy; both show it as from C.
#   3. A answers C; C sees the answer as from the person (not from A), and B
#      gets the sent copy in its history.
#   pip install websocket-client
#   python tools/linked_devices_test.py [target/debug]
import os, struct, subprocess, sys, tempfile, time
import websocket

BIN = sys.argv[1] if len(sys.argv) > 1 else "target/debug"
HOST = os.path.abspath(os.path.join(BIN, "maarse-host" + (".exe" if os.name == "nt" else "")))
TMP = tempfile.mkdtemp(prefix="maarse-linked-test-")
procs = []


def start(name, http, extra):
    d = os.path.join(TMP, name)
    os.makedirs(d, exist_ok=True)
    args = [HOST, "--data", d, "--http", f"127.0.0.1:{http}", "--no-browser", "--no-history"] + extra
    procs.append(subprocess.Popen(args, stdout=open(os.path.join(d, "log.txt"), "a"), stderr=subprocess.STDOUT))


def frame(kind, *parts):
    body = bytes([kind]) + b"".join(parts)
    return struct.pack("<H", len(body)) + body


class Client:
    def __init__(self, port):
        for _ in range(50):
            try:
                self.ws = websocket.create_connection(f"ws://127.0.0.1:{port}/ws", origin=f"http://127.0.0.1:{port}", timeout=5)
                break
            except OSError:
                time.sleep(0.2)
        self.buf, self.events = b"", []
        self.send(frame(0x01))

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

    def wait(self, pred, secs=30, what="event"):
        end = time.time() + secs
        while time.time() < end:
            for f in self.events:
                if pred(f):
                    self.events.remove(f)
                    return f
            self.pump(0.3)
        raise AssertionError("waited in vain for " + what)

    def addr(self):
        f = self.wait(lambda f: f[0] == 0x81, what="Info")
        return f[1:17]


def person(f):
    # 0xb6: state | flags | person (16) | index | version (4) | count | [dev (16) | revoked]... | words
    n = f[24]
    devs = [f[25 + 17 * i:41 + 17 * i] for i in range(n)]
    return {"state": f[1], "master": bool(f[2] & 1), "person": f[3:19], "index": f[19], "devices": devs, "words": f[25 + 17 * n:].decode()}


def direct(f, text):
    return f[0] == 0x83 and f[18:].decode(errors="replace") == text


ok = True
try:
    start("A", 8191, ["--listen", "127.0.0.1:4391"])
    time.sleep(0.5)
    start("B", 8192, ["--listen", "off", "--peer", "127.0.0.1:4391"])
    start("C", 8193, ["--listen", "off", "--peer", "127.0.0.1:4391"])
    a, b, c = Client(8191), Client(8192), Client(8193)
    A, B, C = a.addr(), b.addr(), c.addr()
    print("A", A.hex()[:8], "B", B.hex()[:8], "C", C.hex()[:8])

    # 1. A makes the person, B waits, A links B.
    a.send(frame(0x37, struct.pack("<H", 1), bytes([1])))
    phrase = a.wait(lambda f: f[0] == 0xb7, what="PersonPhrase")[1:].decode()
    assert len(phrase.split()) == 24, phrase
    pa = person(a.wait(lambda f: f[0] == 0xb6 and f[1] == 2, what="A member"))
    M = pa["person"]
    assert pa["master"] and pa["devices"] == [A]
    print("1. person", M.hex()[:8], "made on A (24 words)")
    b.send(frame(0x37, struct.pack("<H", 2), bytes([3])))
    pb = person(b.wait(lambda f: f[0] == 0xb6 and f[1] == 1, what="B awaiting"))
    print("   B waits, words:", pb["words"])
    a.pump(2)
    a.send(frame(0x35, struct.pack("<H", 3), B))
    la = a.wait(lambda f: f[0] == 0xb5, what="A DeviceLinked")
    lb = b.wait(lambda f: f[0] == 0xb5, what="B DeviceLinked")
    assert la[18:].decode() == lb[18:].decode() == pb["words"], (la[18:], lb[18:])
    pb = person(b.wait(lambda f: f[0] == 0xb6 and f[1] == 2, what="B member"))
    assert pb["person"] == M and not pb["master"] and set(pb["devices"]) == {A, B}
    print("   linked; both show", la[18:].decode())

    # Announces so C hears both devices' claims.
    time.sleep(1)
    a.send(frame(0x05)); b.send(frame(0x05))
    c.pump(3)

    # 2. C writes to the person.
    c.send(frame(0x02, struct.pack("<H", 9), M, "Hello person".encode()))
    c.wait(lambda f: f[0] == 0x82, what="C Sent")
    fa = a.wait(lambda f: direct(f, "Hello person"), what="A got it")
    fb = b.wait(lambda f: direct(f, "Hello person"), what="B got it")
    assert fa[1:17] == C and fb[1:17] == C, "shown as from C"
    print("2. C -> person: both A and B have it, from C")

    # 3. A answers C.
    a.send(frame(0x02, struct.pack("<H", 10), C, "Hi from the PC".encode()))
    fc = c.wait(lambda f: direct(f, "Hi from the PC"), what="C got the answer")
    assert fc[1:17] == M, f"C sees it from {fc[1:17].hex()[:8]}, not the person"
    print("3. A -> C: C sees it from the person", M.hex()[:8])
    # B: the sent copy arrives (history is off in this test, so check B's log).
    time.sleep(2)
    log = open(os.path.join(TMP, "B", "log.txt"), encoding="utf-8", errors="replace").read()
    assert "copy from our device" in log and "message to" in log, "B got no sent copy"
    print("   B got the sent copy")
    print("PASSED")
except AssertionError as e:
    ok = False
    print("FAILED:", e)
    for n in "ABC":
        print(f"--- {n} log (tail)")
        print("".join(open(os.path.join(TMP, n, "log.txt"), encoding="utf-8", errors="replace").readlines()[-25:]))
finally:
    for p in procs:
        p.terminate()
sys.exit(0 if ok else 1)
