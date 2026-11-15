# End-to-end test of maarse-host: messages, channel, files and mailbox.
#   pip install websocket-client
#   python tools/e2e_test.py <path to maarse-host>
# Starts three nodes itself on 127.0.0.1 (ports 818x/43xx) in a temporary folder.
import os, struct, subprocess, sys, tempfile, time, hashlib
import websocket

HOST = sys.argv[1] if len(sys.argv) > 1 else "target/release/maarse-host"
procs = {}

def start(name, http, extra):
    d = os.path.join(TMP, name)
    os.makedirs(d, exist_ok=True)
    env = dict(os.environ, MAARSE_DEPOSIT_AFTER_S="3")
    p = subprocess.Popen([os.path.abspath(HOST), "--no-browser", "--data", d, "--http", f"127.0.0.1:{http}"] + extra,
                         stdout=open(os.path.join(d, "log.txt"), "a"), stderr=subprocess.STDOUT, env=env)
    procs[name] = p
    time.sleep(0.6)

def stop(name):
    procs[name].terminate(); procs[name].wait()

def frame(kind, *parts):
    body = bytes([kind]) + b"".join(parts)
    return struct.pack("<H", len(body)) + body

class Client:
    def __init__(self, port):
        self.ws = websocket.create_connection(f"ws://127.0.0.1:{port}/ws", origin=f"http://127.0.0.1:{port}", timeout=5)
        self.buf = b""
        self.events = []
    def send(self, b): self.ws.send_binary(b)
    def pump(self, secs=1.0):
        end = time.time() + secs
        self.ws.settimeout(0.1)
        while time.time() < end:
            try: self.buf += self.ws.recv()
            except websocket.WebSocketTimeoutException: continue
            while len(self.buf) >= 2:
                n = struct.unpack("<H", self.buf[:2])[0]
                if len(self.buf) < 2 + n: break
                self.events.append(self.buf[2:2 + n]); self.buf = self.buf[2 + n:]
    def wait(self, pred, secs=10):
        end = time.time() + secs
        while time.time() < end:
            for f in self.events:
                if pred(f): return f
            self.pump(0.3)
        raise AssertionError("waited in vain")
    def info(self): return self.wait(lambda f: f[0] == 0x81)[1:17]

def ok(msg): print("  OK  " + msg)

def log_has(name, *parts, secs=5):
    """Waits for a line in the node's log (e.g. which encryption format was used)."""
    end = time.time() + secs
    while time.time() < end:
        with open(os.path.join(TMP, name, "log.txt"), encoding="utf-8", errors="replace") as f:
            if any(all(t in line for t in parts) for line in f): return
        time.sleep(0.2)
    raise AssertionError(f"{name}: {parts} missing from the log")

TMP = tempfile.mkdtemp(prefix="maarse-e2e-")
try:
    start("p", 8180, ["--listen", "127.0.0.1:4380"])
    start("a", 8181, ["--listen", "127.0.0.1:4381", "--peer", "127.0.0.1:4380"])
    a, p = Client(8181), Client(8180)
    addr_a, addr_p = a.info(), p.info()
    start("b", 8182, ["--listen", "off", "--peer", "127.0.0.1:4381", "--mailbox", addr_p.hex()])
    b = Client(8182); addr_b = b.info()
    a.wait(lambda f: f[0] == 0x87 and f[1:17] == addr_b)
    b.wait(lambda f: f[0] == 0x87 and f[1:17] == addr_a)

    # 1. Direct message both ways
    a.send(frame(0x02, struct.pack("<H", 1), addr_b, "Hi B – æøå".encode()))
    b.wait(lambda f: f[0] == 0x83 and f[18:] == "Hi B – æøå".encode())
    a.wait(lambda f: f[0] == 0x85)
    ok("direct message A -> B with acknowledgement")
    b.send(frame(0x02, struct.pack("<H", 2), addr_a, b"Hi A"))
    a.wait(lambda f: f[0] == 0x83 and f[18:] == b"Hi A")
    ok("direct message B -> A")

    # 1b. Forward secrecy: the first message starts a session (init), the reply and
    #     everything after uses the ratchet format.
    log_has("b", "message from " + addr_a.hex()[:8], "forward secrecy: init)")
    log_has("a", "message from " + addr_b.hex()[:8], "hops, forward secrecy)")
    a.send(frame(0x02, struct.pack("<H", 10), addr_b, b"Now with ratchet"))
    b.wait(lambda f: f[0] == 0x83 and f[18:] == b"Now with ratchet")
    log_has("a", "message to " + addr_b.hex()[:8] + " (forward secrecy)")
    log_has("b", "message from " + addr_a.hex()[:8], "hops, forward secrecy)")
    ok("direct messages between two hosts use forward secrecy (init, then ratchet)")

    # 1c. History: several screens on the same host. Screen 1 (a) sends, screen 2
    #     (a2) sees it live; screen 3 (a3) connects afterwards and fetches the history.
    #     HistoryItem (0xB0): id | unix | flag | kind | state | hops | msg_id | conv | from | text
    a2 = Client(8181); a2.info()
    a.send(frame(0x02, struct.pack("<H", 40), addr_b, "From screen 1".encode()))
    sent = a.wait(lambda f: f[0] == 0x82 and f[1:3] == struct.pack("<H", 40))
    live = a2.wait(lambda f: f[0] == 0xB0 and f[57:] == "From screen 1".encode())
    assert live[13] & 3 == 3 and live[25:41] == addr_b and live[17:25] == sent[3:11], live
    assert not any(f[0] == 0xB0 and f[57:] == "From screen 1".encode() for f in a.events), "the sender got its own message back"
    b.wait(lambda f: f[0] == 0x83 and f[18:] == "From screen 1".encode())
    a2.wait(lambda f: f[0] == 0x85 and f[1:9] == sent[3:11])
    ok("message sent from screen 1 is shown live on screen 2 (same host)")
    a3 = Client(8181)
    info = a3.wait(lambda f: f[0] == 0xB3)
    assert info[1] == 1 and struct.unpack("<H", info[2:4])[0] == 90, info
    rm = a3.wait(lambda f: f[0] == 0xB2 and f[1:17] == addr_b)
    assert struct.unpack("<H", rm[25:27])[0] >= 1, "B's message should be unread"
    a3.send(frame(0x30, struct.pack("<H", 41), addr_b, struct.pack("<Q", 0), bytes([50])))
    end = a3.wait(lambda f: f[0] == 0xB1 and f[1:3] == struct.pack("<H", 41))
    items = [f for f in a3.events if f[0] == 0xB0 and f[25:41] == addr_b]
    texts = [(f[13] & 1, f[57:]) for f in items]
    assert (0, b"Hi A") in texts and (1, "Hi B – æøå".encode()) in texts and (1, "From screen 1".encode()) in texts, texts
    mine = [f for f in items if f[57:] == "From screen 1".encode()][0]
    assert mine[15] == 2, "the acknowledgement should be stored (state 2)"
    assert end[19] == len(items), (end[19], len(items))
    ok(f"new screen 3 fetches the history ({len(items)} messages with B, incl. acknowledgement)")
    a3.send(frame(0x30, struct.pack("<H", 42), addr_b, struct.pack("<Q", 0), bytes([1])))
    e1 = a3.wait(lambda f: f[0] == 0xB1 and f[1:3] == struct.pack("<H", 42))
    assert e1[19] == 1 and e1[20] == 1, "page 1 should have more"
    oldest = e1[21:29]
    a3.send(frame(0x30, struct.pack("<H", 43), addr_b, oldest, bytes([200])))
    e2 = a3.wait(lambda f: f[0] == 0xB1 and f[1:3] == struct.pack("<H", 43))
    assert e2[19] == len(items) - 1 and e2[20] == 0, (e2[19], e2[20])
    ok("history in pages (before id)")
    a3.send(frame(0x31, addr_b, struct.pack("<Q", 0)))
    for c in (a, a2, a3):
        m = c.wait(lambda f: f[0] == 0xB2 and f[1:17] == addr_b and f[25:27] == bytes(2))
    ok("read on screen 3 is sent to all screens (unread = 0)")

    # 2. Channel
    for c in (a, b): c.send(frame(0x04, b"#test"))
    j = a.wait(lambda f: f[0] == 0x88 and f[17:] == b"#test")
    b.wait(lambda f: f[0] == 0x88 and f[17:] == b"#test")
    a.send(frame(0x03, struct.pack("<H", 3), j[1:17], b"Hi channel"))
    b.wait(lambda f: f[0] == 0x84 and f[34:] == b"Hi channel")
    ok("channel message")

    # 2b. Private channel with a random key (command 0x13)
    key = os.urandom(32)
    for c in (a, b): c.send(frame(0x13, key, "Cabin æ".encode()))
    pj = a.wait(lambda f: f[0] == 0x88 and f[17:] == "Cabin æ".encode())
    assert pj[1:17] == hashlib.sha256(key).digest()[:16], "wrong channel address"
    b.wait(lambda f: f[0] == 0x88 and f[17:] == "Cabin æ".encode())
    a.send(frame(0x03, struct.pack("<H", 9), pj[1:17], b"Secret"))
    b.wait(lambda f: f[0] == 0x84 and f[1:17] == pj[1:17] and f[34:] == b"Secret")
    ok("private channel message")

    # 2c. Diagnostics: trace route B -> P goes via A (B is only connected to A).
    b.send(frame(0x20, struct.pack("<H", 30), addr_p))
    tr = b.wait(lambda f: f[0] == 0xA0 and f[1:3] == struct.pack("<H", 30), secs=15)
    assert tr[3:19] == addr_p
    k = tr[19]
    ids = [struct.unpack("<H", tr[20 + 2 * i:22 + 2 * i])[0] for i in range(k)]
    m = tr[20 + 2 * k]
    snr_out, snr_back = tr[21 + 2 * k:21 + 2 * k + m], tr[21 + 2 * k + m:]
    assert ids == [int.from_bytes(addr_a[:2], "big")], ids
    assert len(snr_out) == 2 and len(snr_back) == 2, (snr_out, snr_back)
    ok(f"trace route B -> P via A (SNR out {list(snr_out)}, back {list(snr_back)})")

    # 2d. Neighbour list: A has heard B and P directly.
    a.send(frame(0x21))
    nb = a.wait(lambda f: f[0] == 0xA1 and f[2] == 1)
    prefixes = [nb[2 + 15 * i + 3:2 + 15 * i + 7] for i in range((len(nb) - 3) // 15)]
    assert addr_b[:4] in prefixes and addr_p[:4] in prefixes, prefixes
    ok(f"neighbour list at A: {len(prefixes)} neighbours, incl. B and P")

    # 2e. Position (opt-in, ~1 km) and telemetry in announces, plus MQTT bridge
    #     to a small mock broker.
    import socket, threading
    published = []
    srv = socket.socket(); srv.bind(("127.0.0.1", 0)); srv.listen(1)
    mqtt_port = srv.getsockname()[1]
    def broker():
        c, _ = srv.accept(); c.settimeout(30); buf = b""
        def take():
            nonlocal buf
            while True:
                if len(buf) >= 2:
                    ln, mul, i = 0, 1, 1
                    while i < len(buf) and i < 5:
                        ln += (buf[i] & 0x7f) * mul; mul *= 128
                        if buf[i] & 0x80 == 0: break
                        i += 1
                    if i < len(buf) and len(buf) >= i + 1 + ln:
                        pkt, buf = buf[:i + 1 + ln], buf[i + 1 + ln:]
                        return pkt[0], pkt[i + 1:]
                d = c.recv(4096)
                if not d: raise EOFError
                buf += d
        try:
            kind, _ = take(); assert kind == 0x10
            c.sendall(bytes([0x20, 2, 0, 0]))
            while True:
                kind, body = take()
                if kind & 0xF0 == 0x30:
                    tl = struct.unpack(">H", body[:2])[0]
                    published.append((body[2:2 + tl].decode(), body[2 + tl:].decode()))
        except Exception:
            pass
    threading.Thread(target=broker, daemon=True).start()
    start("m", 8183, ["--listen", "off", "--peer", "127.0.0.1:4381", "--mqtt", f"127.0.0.1:{mqtt_port}"])
    mc = Client(8183); mc.info()
    a.send(frame(0x23))
    sh = a.wait(lambda f: f[0] == 0xA2)
    assert sh[1] == 0, "position must be off by default"
    a.send(frame(0x22, bytes([2]), struct.pack("<ii", 561_567_000, 102_107_000), bytes([1])))
    a.wait(lambda f: f[0] == 0xA2 and f[1] == 2)
    ni = b.wait(lambda f: f[0] == 0xA3 and f[1:17] == addr_a, secs=10)
    lat, lon = struct.unpack("<ii", ni[21:29])
    assert ni[20] == 2 and (lat, lon) == (561_550_000, 102_150_000), (ni[20], lat, lon)
    ok("A shares position (rounded to a ~1 km cell) and telemetry; B sees it")
    end = time.time() + 10
    want = f"maarse/node/{addr_a.hex()}/telemetry"
    while time.time() < end and not any(t == want for t, _ in published): time.sleep(0.2)
    topics = [t for t, _ in published]
    assert "maarse/status" in topics and want in topics, topics
    assert any(t.startswith("homeassistant/sensor/maarse_" + addr_a.hex()[:8]) for t in topics), topics
    assert any(t == f"maarse/node/{addr_a.hex()}/position" for t in topics), topics
    a.send(frame(0x22, bytes([0]), struct.pack("<ii", 0, 0), bytes([0])))  # turn sharing off again
    ok(f"MQTT bridge: {len(published)} topics published (status, Home Assistant discovery, telemetry, position)")

    # 3. File (20 KB, 110 chunks)
    data = os.urandom(20_000)
    a.send(frame(0x06, struct.pack("<H", 4), addr_b, struct.pack("<I", len(data)), bytes([10]), b"image/webp", "test æ.webp".encode()))
    for off in range(0, len(data), 240):
        a.send(frame(0x07, struct.pack("<H", 4), struct.pack("<I", off), data[off:off + 240]))
    a.send(frame(0x08, struct.pack("<H", 4)))
    started = a.wait(lambda f: f[0] == 0x8d)
    fid = started[3:11]
    t0 = time.time()
    b.wait(lambda f: f[0] == 0x8c and f[1:9] == fid, secs=60)
    done = [f for f in b.events if f[0] == 0x8c and f[1:9] == fid][0]
    assert done[9] == 1, "received file failed: " + done[11:].decode()
    got = b"".join(f[13:] for f in sorted((f for f in b.events if f[0] == 0x8a and f[1:9] == fid), key=lambda f: struct.unpack("<I", f[9:13])[0]))
    assert hashlib.sha256(got).digest() == hashlib.sha256(data).digest()
    a.wait(lambda f: f[0] == 0x8c and f[1:9] == fid and f[9] == 1)
    ok(f"20 KB file transferred and verified ({time.time() - t0:.1f} s)")

    # 4. Too large a file is rejected
    a.send(frame(0x06, struct.pack("<H", 5), addr_b, struct.pack("<I", 5_000_000), bytes([4]), b"x/y", b"large"))
    a.wait(lambda f: f[0] == 0x8f and f[1:3] == struct.pack("<H", 5))
    ok("too large file rejected")

    # 5. Remote administration: A is an administrator of P, B is not
    p.send(frame(0x0d, addr_a))
    p.wait(lambda f: f[0] == 0x94 and f[1:17] == addr_a)
    a.send(frame(0x0c, struct.pack("<H", 7), addr_p, bytes([0x09])))
    rr = a.wait(lambda f: f[0] == 0x93 and f[1:17] == addr_p and f[17] == 0x91, secs=10)
    items = {rr[18 + i]: struct.unpack("<i", rr[19 + i:23 + i])[0] for i in range(0, len(rr) - 18, 5)}
    assert items.get(1, -1) >= 0 and 2 in items, items
    ok(f"remote status from P fetched by administrator A ({len(items)} values in the first part)")
    p.send(frame(0x05))  # P announces itself so B knows P's key
    b.wait(lambda f: f[0] == 0x87 and f[1:17] == addr_p)
    b.send(frame(0x0c, struct.pack("<H", 8), addr_p, bytes([0x09])))
    rb = b.wait(lambda f: f[0] == 0x93 and f[1:17] == addr_p, secs=10)
    assert rb[17] == 0x8f and b"not an administrator" in rb, rb
    ok("remote status rejected for B, which is not an administrator")

    # 6. Mailbox: B goes offline, A writes, B comes back
    stop("b"); time.sleep(1)
    a.send(frame(0x02, struct.pack("<H", 6), addr_b, b"For you, while you were away"))
    sent = a.wait(lambda f: f[0] == 0x82 and f[1:3] == struct.pack("<H", 6))
    a.wait(lambda f: f[0] == 0x90 and f[1:9] == sent[3:11], secs=20)
    ok("B offline: message put in B's mailbox at P")
    start("b", 8182, ["--listen", "off", "--peer", "127.0.0.1:4381", "--mailbox", addr_p.hex()])
    b = Client(8182)
    b.wait(lambda f: f[0] == 0x83 and f[18:] == b"For you, while you were away", secs=20)
    ok("B online again: the message fetched from the mailbox")
    # B's session survived the restart (sessions.bin), so the mail was FS.
    with open(os.path.join(TMP, "b", "log.txt"), encoding="utf-8", errors="replace") as f:
        lines = [l for l in f if "message from " + addr_a.hex()[:8] in l]
    assert "forward secrecy)" in lines[-1], lines[-1]
    ok("mail from the mailbox opened with the stored forward secrecy session")
    b.wait(lambda f: f[0] == 0x88 and f[1:17] == pj[1:17] and f[17:] == "Cabin æ".encode())
    ok("B remembers the private channel after a restart")

    # 6b. File mailbox: B goes offline, A sends a 20 KB file, it is put in
    #     A's file mailbox and delivered over TCP when B comes back.
    def file_limit(c, dest):
        c.events.clear()
        c.send(frame(0x14, dest))
        f = c.wait(lambda f: f[0] == 0x96 and f[1:17] == dest)
        return struct.unpack("<I", f[17:21])[0], f[21], struct.unpack("<I", f[22:26])[0]
    def send_file(c, req, dest, data, name=b"mail.webp"):
        c.send(frame(0x06, struct.pack("<H", req), dest, struct.pack("<I", len(data)), bytes([10]), b"image/webp", name))
        for off in range(0, len(data), 240):
            c.send(frame(0x07, struct.pack("<H", req), struct.pack("<I", off), data[off:off + 240]))
        c.send(frame(0x08, struct.pack("<H", req)))
    stop("b"); time.sleep(1)
    mx, flags, _ = file_limit(a, addr_b)
    assert mx == 1 << 20 and flags & 2, (mx, flags)
    ok("B offline: GetFileLimit says mailbox, up to 1 MB")
    pdata = os.urandom(20_000)
    send_file(a, 30, addr_b, pdata)
    fid = a.wait(lambda f: f[0] == 0x8d and f[1:3] == struct.pack("<H", 30))[3:11]
    a.wait(lambda f: f[0] == 0x8c and f[1:9] == fid and f[9] == 1 and f[11:] == b"put in mailbox", secs=20)
    ok("20 KB file put in A's file mailbox")
    start("b", 8182, ["--listen", "off", "--peer", "127.0.0.1:4381", "--mailbox", addr_p.hex()])
    b = Client(8182)
    b.wait(lambda f: f[0] == 0x8c and f[1:9] == fid, secs=40)
    done = [f for f in b.events if f[0] == 0x8c and f[1:9] == fid][0]
    assert done[9] == 1, "file from the mailbox failed: " + done[11:].decode()
    got = b"".join(f[13:] for f in sorted((f for f in b.events if f[0] == 0x8a and f[1:9] == fid), key=lambda f: struct.unpack("<I", f[9:13])[0]))
    assert hashlib.sha256(got).digest() == hashlib.sha256(pdata).digest()
    a.wait(lambda f: f[0] == 0x8c and f[1:9] == fid and f[11:] == b"delivered from mailbox", secs=20)
    ok("B online again: the file delivered from the file mailbox and verified")
    mx, flags, _ = file_limit(a, addr_b)
    assert mx == 1 << 20 and flags == 0, (mx, flags)
    ok("B online over TCP: up to 1 MB, not over LoRa")

    # 6c. Over the limit: without a file mailbox and without knowledge of the route it counts
    #     as LoRa (conservatively): 8 KB, and a 20 KB file is rejected.
    start("c", 8184, ["--listen", "off", "--peer", "127.0.0.1:4381", "--file-mailbox-mb", "0"])
    c = Client(8184); c.info()
    unknown = os.urandom(16)
    mx, flags, air = file_limit(c, unknown)
    assert mx == 8192 and flags == 1 and air > 0, (mx, flags, air)
    ok(f"unknown route without a file mailbox: 8 KB over LoRa (about {air} s airtime)")
    c.send(frame(0x06, struct.pack("<H", 31), unknown, struct.pack("<I", 20_000), bytes([10]), b"image/webp", b"large.webp"))
    e = c.wait(lambda f: f[0] == 0x8f and f[1:3] == struct.pack("<H", 31))
    assert e[3] == 7 and "LoRa" in e[4:].decode(), e
    stop("c")
    ok("20 KB file over LoRa rejected: " + e[4:].decode())

    # 7. Wrong origin
    try:
        websocket.create_connection("ws://127.0.0.1:8181/ws", origin="http://evil.example", timeout=3)
        raise AssertionError("wrong origin was accepted")
    except websocket.WebSocketBadStatusException:
        ok("WebSocket from a foreign page rejected")

    # 8. Regions over the backbone: H (dk + nordjylland) with two peers,
    #    N (nordjylland) and K (dk). Nordjylland messages only reach N.
    start("h", 8190, ["--listen", "127.0.0.1:4390", "--region", "dk", "--region", "nordjylland"])
    start("n", 8191, ["--listen", "off", "--peer", "127.0.0.1:4390", "--region", "nordjylland"])
    start("k", 8192, ["--listen", "off", "--peer", "127.0.0.1:4390", "--region", "dk"])
    h, n, k = Client(8190), Client(8191), Client(8192)
    for c in (h, n, k): c.info()
    h.send(frame(0x12))
    r = h.wait(lambda f: f[0] == 0x95)
    assert r[1] == 0 and r[2:] == b"dk,nordjylland", r
    ok("regions set with --region and fetched with GetRegions")
    time.sleep(1.5)  # region hellos are exchanged
    for c in (n, k): c.send(frame(0x04, b"#hike"))
    h.send(frame(0x04, b"#hike@nordjylland"))
    j = h.wait(lambda f: f[0] == 0x88 and f[17:] == b"#hike@nordjylland")
    h.send(frame(0x03, struct.pack("<H", 20), j[1:17], b"Only Nordjylland"))
    n.wait(lambda f: f[0] == 0x84 and f[34:] == b"Only Nordjylland")
    k.pump(2)
    assert not any(f[0] == 0x84 and f[34:] == b"Only Nordjylland" for f in k.events), "K got a Nordjylland message"
    ok("#hike@nordjylland reaches N, but not K")
    h.send(frame(0x04, b"#hike@dk"))
    h.wait(lambda f: f[0] == 0x88 and f[17:] == b"#hike@dk")
    h.send(frame(0x03, struct.pack("<H", 21), j[1:17], b"All of Denmark"))
    k.wait(lambda f: f[0] == 0x84 and f[34:] == b"All of Denmark")
    n.pump(2)
    assert not any(f[0] == 0x84 and f[34:] == b"All of Denmark" for f in n.events), "N (only nordjylland) got a dk message"
    ok("#hike@dk reaches K, but not N (which only serves nordjylland)")
    # Empty list and strict via the companion command.
    n.send(frame(0x11, bytes([1]), b""))
    r = n.wait(lambda f: f[0] == 0x95 and f[1] == 1)
    ok("SetRegions (strict, empty list) answered")

    # Firmware update (0x1b OtaStatus): maarse-host is not updated over
    # the air and answers "not supported" (state 5) in OtaState (0x9a).
    n.send(frame(0x1b))
    n.wait(lambda f: f[0] == 0x9a and f[3] == 5)
    ok("OtaStatus answered (maarse-host: not supported)")

    # 9. Recovery phrase: SetIdentity (0x34) with the master secret for
    #    "abandon … art" (without passphrase, see maarse-crypto/src/recovery.rs).
    MASTER = bytes.fromhex("743d6ccda8a1826db3ef1df490b9d6d82f4b3920c76335447f21b261e4bf7252"
                           "5485ca54b88ea0c0358d84254ee7c92855aa5ec5fec21de960bf6f00996c80a4")
    WANT = bytes.fromhex("6ef182b74279e05122977ced600ae3e6")
    start("i", 8193, ["--listen", "off"])
    i = Client(8193); old = i.info()
    i.send(frame(0x34, bytes([0]), MASTER))
    i.wait(lambda f: f[0] == 0x8f)
    assert i.info() == old
    ok("SetIdentity without confirmation rejected")
    i.send(frame(0x0c, struct.pack("<H", 30), old, frame(0x34, bytes([1]), MASTER)[2:]))
    i.wait(lambda f: f[0] == 0x8f and b"controlled remotely" in f[4:])
    ok("SetIdentity via Remote rejected")
    i.events.clear()
    i.send(frame(0x34, bytes([1]), MASTER))
    ch = i.wait(lambda f: f[0] == 0xb4)
    assert ch[1:17] == WANT, ch[1:17].hex()
    assert hashlib.sha256(ch[17:81]).digest()[:16] == WANT
    assert i.wait(lambda f: f[0] == 0x81)[1:17] == WANT
    with open(os.path.join(TMP, "i", "identity.key"), "rb") as f: assert f.read() == MASTER
    assert os.path.getsize(os.path.join(TMP, "i", "identity.old")) == 64
    ok("SetIdentity: the address is the expected one, and the old key is in identity.old")
    i.ws.close(); stop("i"); start("i", 8193, ["--listen", "off"])
    i = Client(8193)
    assert i.info() == WANT
    ok("the identity from the phrase survives a restart")
    print("ALL TESTS OK")
finally:
    for n in list(procs): 
        try: stop(n)
        except Exception: pass
