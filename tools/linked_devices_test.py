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
procs = {}


def start(name, http, extra, history=False):
    d = os.path.join(TMP, name)
    os.makedirs(d, exist_ok=True)
    args = [HOST, "--data", d, "--http", f"127.0.0.1:{http}", "--no-browser"] + ([] if history else ["--no-history"]) + extra
    # The friend's host keeps unanswered mail after 3 s instead of 45 s.
    env = dict(os.environ, MAARSE_DEPOSIT_AFTER_S="3")
    procs[name] = subprocess.Popen(args, stdout=open(os.path.join(d, "log.txt"), "a"), stderr=subprocess.STDOUT, env=env)


def stop(name):
    procs[name].terminate()
    procs[name].wait()


def log_of(name):
    return open(os.path.join(TMP, name, "log.txt"), encoding="utf-8", errors="replace").read()


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
    start("A", 8191, ["--listen", "127.0.0.1:4391"], history=True)
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

    # 3a. A channel: the phone writes; C sees one person (not device B) and
    #     A shows it as its own message instead of a message from "B".
    for cl in (a, b, c):
        cl.send(frame(0x04, b"#linked"))
    ch = a.wait(lambda f: f[0] == 0x88 and f[17:] == b"#linked")[1:17]
    b.wait(lambda f: f[0] == 0x88 and f[17:] == b"#linked")
    c.wait(lambda f: f[0] == 0x88 and f[17:] == b"#linked")
    time.sleep(1)
    b.send(frame(0x03, struct.pack("<H", 12), ch, b"From the phone"))
    fc = c.wait(lambda f: f[0] == 0x84 and f[34:] == b"From the phone", what="C got the channel message")
    assert fc[17:33] == M, f"C sees the channel message from {fc[17:33].hex()[:8]}, not the person"
    a.wait(lambda f: f[0] == 0xb0 and f[57:].decode(errors="replace") == "From the phone", what="A shows it as its own")
    assert not any(f[0] == 0x84 and f[34:] == b"From the phone" for f in a.events), "A shows the phone's message as from another"
    print("3a. channel: C sees the person; A shows the phone's message as its own")

    # 3b. A new device E (with history) is linked and syncs: it gets the
    #     earlier messages from A's history, without notifications.
    start("E", 8195, ["--listen", "off", "--peer", "127.0.0.1:4391"], history=True)
    e = Client(8195)
    E = e.addr()
    e.send(frame(0x37, struct.pack("<H", 20), bytes([3])))
    e.wait(lambda f: f[0] == 0xb6 and f[1] == 1, what="E awaiting")
    a.pump(2)
    a.send(frame(0x35, struct.pack("<H", 21), E))
    e.wait(lambda f: f[0] == 0xb6 and f[1] == 2, what="E member")
    e.send(frame(0x37, struct.pack("<H", 22), bytes([9])))
    hist = lambda f, t: f[0] == 0xb0 and f[57:].decode(errors="replace") == t
    e.wait(lambda f: hist(f, "Hello person"), secs=30, what="E got the old message by sync")
    e.wait(lambda f: hist(f, "Hi from the PC"), secs=30, what="E got the old sent message by sync")
    assert not any(f[0] == 0x83 for f in e.events), "a synced message must not look new"
    print("3b. E linked and synced: it has the earlier messages from A's history")

    # 3c. The user writes from E; then C writes to the person and it reaches
    #     A: A tells C to write to E next time, and the next one goes to E.
    c.send(frame(0x05)); e.pump(2)
    e.send(frame(0x02, struct.pack("<H", 23), C, "Now on E".encode()))
    c.wait(lambda f: direct(f, "Now on E"), what="C got E's message")
    time.sleep(1)
    a.send(frame(0x05)); time.sleep(1); c.pump(1)
    c.send(frame(0x02, struct.pack("<H", 24), M, "To the person, lands on A".encode()))
    a.wait(lambda f: direct(f, "To the person, lands on A"), what="A got it")
    for _ in range(30):
        if "for " + M.hex()[:8] + " next time" in log_of("C"):
            break
        time.sleep(0.5)
    assert "write to " + E.hex()[:8] in log_of("C"), "C got no redirect hint"
    c.send(frame(0x02, struct.pack("<H", 25), M, "Next one".encode()))
    e.wait(lambda f: direct(f, "Next one"), what="E got the next one")
    assert "via device " + E.hex()[:8] in log_of("C").split("To the person, lands on A")[-1] or "via device " + E.hex()[:8] in log_of("C")
    print("3c. redirect hint: A told C the user is on E; the next message went to E")

    # 4. The phone is off; C writes to the person. C's host keeps it under the
    #    person (inbox key) and hands it to A when A announces.
    stop("B")
    stop("E")
    time.sleep(1)
    b.ws.close()
    e.ws.close()
    c.pump(1)
    # C picks E (the redirect made it the newest): no answer -> deposit under M.
    c.send(frame(0x02, struct.pack("<H", 11), M, "While the phone is off".encode()))
    c.wait(lambda f: f[0] == 0x90, secs=30, what="C Deposited")
    assert "post office: stored message for " + M.hex()[:8] in log_of("C"), "not kept under the person"
    print("4. phone off: C keeps the message under the person", M.hex()[:8])
    a.send(frame(0x05))
    fa = a.wait(lambda f: direct(f, "While the phone is off"), secs=40, what="A got the kept mail")
    assert fa[1:17] == C
    print("   A announced and got it from C's post office (inbox key), from C")
    # The phone comes back: A's sibling copy (kept for B) reaches it.
    start("B", 8192, ["--listen", "off", "--peer", "127.0.0.1:4391"])
    b = Client(8192)
    b.wait(lambda f: f[0] == 0x81, what="B Info")
    b.send(frame(0x05))
    time.sleep(1)
    a.send(frame(0x05))
    fb = b.wait(lambda f: direct(f, "While the phone is off"), secs=60, what="B got the copy after coming back")
    assert fb[1:17] == C
    print("   B came back and got the sibling copy")

    # 5. A removes B: B learns it is out; the inbox chain changes.
    a.send(frame(0x37, struct.pack("<H", 12), bytes([6]), B.hex().encode()))
    pa = person(a.wait(lambda f: f[0] == 0xb6 and f[1] == 2 and len(f) > 25 and f[25 + 16 + 17 * 0] in (0, 1), what="A list after removal"))
    b.wait(lambda f: f[0] == 0xb6 and f[1] == 0, secs=30, what="B told it was removed")
    assert "new inbox chain" in log_of("A")
    print("5. A removed B; B knows it is out, new inbox chain on A")

    # 6. Backup v2: A writes a backup (6-word code); a new device D restores
    #    from it and becomes the master; the old master A is removed.
    a.send(frame(0x37, struct.pack("<H", 13), bytes([7])))
    code = a.wait(lambda f: f[0] == 0xb8, secs=60, what="backup code")[1:].decode()
    assert len(code.split()) == 6, code
    import urllib.request
    req = urllib.request.Request("http://127.0.0.1:8191/api/account-backup", headers={"Origin": "http://127.0.0.1:8191"})
    blob = urllib.request.urlopen(req).read()
    assert blob[:4] == b"MAB2", blob[:8]
    print("6. backup made on A; code has 6 words; file over HTTP:", len(blob), "bytes")
    start("D", 8194, ["--listen", "off", "--peer", "127.0.0.1:4391"])
    d = Client(8194)
    D = d.addr()
    up = urllib.request.Request("http://127.0.0.1:8194/api/account-restore", data=blob, method="POST", headers={"Origin": "http://127.0.0.1:8194"})
    assert urllib.request.urlopen(up).read() == b"ok"
    d.send(frame(0x37, struct.pack("<H", 14), bytes([8]), b"wrong code words here so no"))
    d.wait(lambda f: f[0] == 0x8f, secs=60, what="wrong code refused")
    urllib.request.urlopen(urllib.request.Request("http://127.0.0.1:8194/api/account-restore", data=blob, method="POST", headers={"Origin": "http://127.0.0.1:8194"}))
    d.send(frame(0x37, struct.pack("<H", 15), bytes([8]), code.upper().encode()))
    pd = person(d.wait(lambda f: f[0] == 0xb6 and f[1] == 2, secs=60, what="D master"))
    assert pd["master"] and pd["person"] == M and D in pd["devices"], pd
    print("   D restored the person from the file and the code; D is the master device")
    a.wait(lambda f: f[0] == 0xb6 and f[1] == 0, secs=30, what="A told it was removed")
    print("   the old master A was removed")
    print("PASSED")
except AssertionError as e:
    ok = False
    print("FAILED:", e)
    for n in "ABC":
        print(f"--- {n} log (tail)")
        print("".join(open(os.path.join(TMP, n, "log.txt"), encoding="utf-8", errors="replace").readlines()[-25:]))
finally:
    for p in procs.values():
        p.terminate()
sys.exit(0 if ok else 1)
