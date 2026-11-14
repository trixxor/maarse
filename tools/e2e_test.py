# Ende-til-ende-test af maarse-host: beskeder, kanal, filer og postkasse.
#   pip install websocket-client
#   python tools/e2e_test.py <sti til maarse-host>
# Starter selv tre noder på 127.0.0.1 (porte 818x/43xx) i en midlertidig mappe.
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
        raise AssertionError("ventede forgæves")
    def info(self): return self.wait(lambda f: f[0] == 0x81)[1:17]

def ok(msg): print("  OK  " + msg)

def log_has(name, *parts, secs=5):
    """Venter på en linje i nodens log (fx hvilket krypteringsformat der blev brugt)."""
    end = time.time() + secs
    while time.time() < end:
        with open(os.path.join(TMP, name, "log.txt"), encoding="utf-8", errors="replace") as f:
            if any(all(t in line for t in parts) for line in f): return
        time.sleep(0.2)
    raise AssertionError(f"{name}: {parts} mangler i loggen")

TMP = tempfile.mkdtemp(prefix="maarse-e2e-")
try:
    start("p", 8180, ["--listen", "127.0.0.1:4380"])
    start("a", 8181, ["--listen", "127.0.0.1:4381", "--peer", "127.0.0.1:4380"])
    a, p = Client(8181), Client(8180)
    addr_a, addr_p = a.info(), p.info()
    start("b", 8182, ["--listen", "off", "--peer", "127.0.0.1:4381", "--postkasse", addr_p.hex()])
    b = Client(8182); addr_b = b.info()
    a.wait(lambda f: f[0] == 0x87 and f[1:17] == addr_b)
    b.wait(lambda f: f[0] == 0x87 and f[1:17] == addr_a)

    # 1. Direkte besked begge veje
    a.send(frame(0x02, struct.pack("<H", 1), addr_b, "Hej B – æøå".encode()))
    b.wait(lambda f: f[0] == 0x83 and f[18:] == "Hej B – æøå".encode())
    a.wait(lambda f: f[0] == 0x85)
    ok("direkte besked A -> B med kvittering")
    b.send(frame(0x02, struct.pack("<H", 2), addr_a, b"Hej A"))
    a.wait(lambda f: f[0] == 0x83 and f[18:] == b"Hej A")
    ok("direkte besked B -> A")

    # 1b. Forward secrecy: første besked starter en session (init), svaret og
    #     alt derefter bruger ratchet-formatet.
    log_has("b", "besked fra " + addr_a.hex()[:8], "forward secrecy: init)")
    log_has("a", "besked fra " + addr_b.hex()[:8], "hop, forward secrecy)")
    a.send(frame(0x02, struct.pack("<H", 10), addr_b, b"Nu med ratchet"))
    b.wait(lambda f: f[0] == 0x83 and f[18:] == b"Nu med ratchet")
    log_has("a", "besked til " + addr_b.hex()[:8] + " (forward secrecy)")
    log_has("b", "besked fra " + addr_a.hex()[:8], "hop, forward secrecy)")
    ok("direkte beskeder mellem to hosts bruger forward secrecy (init, så ratchet)")

    # 2. Kanal
    for c in (a, b): c.send(frame(0x04, b"#test"))
    j = a.wait(lambda f: f[0] == 0x88 and f[17:] == b"#test")
    b.wait(lambda f: f[0] == 0x88 and f[17:] == b"#test")
    a.send(frame(0x03, struct.pack("<H", 3), j[1:17], b"Hej kanal"))
    b.wait(lambda f: f[0] == 0x84 and f[34:] == b"Hej kanal")
    ok("kanalbesked")

    # 2b. Privat kanal med tilfældig nøgle (kommando 0x13)
    key = os.urandom(32)
    for c in (a, b): c.send(frame(0x13, key, "Hytten æ".encode()))
    pj = a.wait(lambda f: f[0] == 0x88 and f[17:] == "Hytten æ".encode())
    assert pj[1:17] == hashlib.sha256(key).digest()[:16], "forkert kanaladresse"
    b.wait(lambda f: f[0] == 0x88 and f[17:] == "Hytten æ".encode())
    a.send(frame(0x03, struct.pack("<H", 9), pj[1:17], b"Hemmeligt"))
    b.wait(lambda f: f[0] == 0x84 and f[1:17] == pj[1:17] and f[34:] == b"Hemmeligt")
    ok("privat kanalbesked")

    # 2c. Diagnose: trace route B -> P går via A (B er kun forbundet til A).
    b.send(frame(0x20, struct.pack("<H", 30), addr_p))
    tr = b.wait(lambda f: f[0] == 0xA0 and f[1:3] == struct.pack("<H", 30), secs=15)
    assert tr[3:19] == addr_p
    k = tr[19]
    ids = [struct.unpack("<H", tr[20 + 2 * i:22 + 2 * i])[0] for i in range(k)]
    m = tr[20 + 2 * k]
    snr_out, snr_back = tr[21 + 2 * k:21 + 2 * k + m], tr[21 + 2 * k + m:]
    assert ids == [int.from_bytes(addr_a[:2], "big")], ids
    assert len(snr_out) == 2 and len(snr_back) == 2, (snr_out, snr_back)
    ok(f"trace route B -> P via A (SNR ud {list(snr_out)}, tilbage {list(snr_back)})")

    # 2d. Naboliste: A har hørt B og P direkte.
    a.send(frame(0x21))
    nb = a.wait(lambda f: f[0] == 0xA1 and f[2] == 1)
    prefixes = [nb[2 + 15 * i + 3:2 + 15 * i + 7] for i in range((len(nb) - 3) // 15)]
    assert addr_b[:4] in prefixes and addr_p[:4] in prefixes, prefixes
    ok(f"naboliste hos A: {len(prefixes)} naboer, inkl. B og P")

    # 2e. Position (opt-in, ~1 km) og telemetri i announces, plus MQTT-bro
    #     til en lille mock-broker.
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
    assert sh[1] == 0, "position skal være slukket som standard"
    a.send(frame(0x22, bytes([2]), struct.pack("<ii", 561_567_000, 102_107_000), bytes([1])))
    a.wait(lambda f: f[0] == 0xA2 and f[1] == 2)
    ni = b.wait(lambda f: f[0] == 0xA3 and f[1:17] == addr_a, secs=10)
    lat, lon = struct.unpack("<ii", ni[21:29])
    assert ni[20] == 2 and (lat, lon) == (561_550_000, 102_150_000), (ni[20], lat, lon)
    ok("A deler position (afrundet til ~1 km-celle) og telemetri; B ser den")
    end = time.time() + 10
    want = f"maarse/node/{addr_a.hex()}/telemetry"
    while time.time() < end and not any(t == want for t, _ in published): time.sleep(0.2)
    topics = [t for t, _ in published]
    assert "maarse/status" in topics and want in topics, topics
    assert any(t.startswith("homeassistant/sensor/maarse_" + addr_a.hex()[:8]) for t in topics), topics
    assert any(t == f"maarse/node/{addr_a.hex()}/position" for t in topics), topics
    a.send(frame(0x22, bytes([0]), struct.pack("<ii", 0, 0), bytes([0])))  # slå deling fra igen
    ok(f"MQTT-bro: {len(published)} emner sendt (status, Home Assistant discovery, telemetri, position)")

    # 3. Fil (20 KB, 110 bidder)
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
    assert done[9] == 1, "modtaget fil fejlede: " + done[11:].decode()
    got = b"".join(f[13:] for f in sorted((f for f in b.events if f[0] == 0x8a and f[1:9] == fid), key=lambda f: struct.unpack("<I", f[9:13])[0]))
    assert hashlib.sha256(got).digest() == hashlib.sha256(data).digest()
    a.wait(lambda f: f[0] == 0x8c and f[1:9] == fid and f[9] == 1)
    ok(f"fil på 20 KB overført og verificeret ({time.time() - t0:.1f} s)")

    # 4. For stor fil afvises
    a.send(frame(0x06, struct.pack("<H", 5), addr_b, struct.pack("<I", 5_000_000), bytes([4]), b"x/y", b"stor"))
    a.wait(lambda f: f[0] == 0x8f and f[1:3] == struct.pack("<H", 5))
    ok("for stor fil afvist")

    # 5. Fjernadministration: A er administrator for P, B er ikke
    p.send(frame(0x0d, addr_a))
    p.wait(lambda f: f[0] == 0x94 and f[1:17] == addr_a)
    a.send(frame(0x0c, struct.pack("<H", 7), addr_p, bytes([0x09])))
    rr = a.wait(lambda f: f[0] == 0x93 and f[1:17] == addr_p and f[17] == 0x91, secs=10)
    items = {rr[18 + i]: struct.unpack("<i", rr[19 + i:23 + i])[0] for i in range(0, len(rr) - 18, 5)}
    assert items.get(1, -1) >= 0 and 2 in items, items
    ok(f"fjern status fra P hentet af administrator A ({len(items)} værdier i første del)")
    p.send(frame(0x05))  # P annoncerer sig, så B kender P's nøgle
    b.wait(lambda f: f[0] == 0x87 and f[1:17] == addr_p)
    b.send(frame(0x0c, struct.pack("<H", 8), addr_p, bytes([0x09])))
    rb = b.wait(lambda f: f[0] == 0x93 and f[1:17] == addr_p, secs=10)
    assert rb[17] == 0x8f and b"ikke administrator" in rb, rb
    ok("fjern status afvist for B, som ikke er administrator")

    # 6. Postkasse: B går offline, A skriver, B kommer tilbage
    stop("b"); time.sleep(1)
    a.send(frame(0x02, struct.pack("<H", 6), addr_b, b"Til dig, mens du var vaek"))
    sent = a.wait(lambda f: f[0] == 0x82 and f[1:3] == struct.pack("<H", 6))
    a.wait(lambda f: f[0] == 0x90 and f[1:9] == sent[3:11], secs=20)
    ok("B offline: besked lagt i B's postkasse hos P")
    start("b", 8182, ["--listen", "off", "--peer", "127.0.0.1:4381", "--postkasse", addr_p.hex()])
    b = Client(8182)
    b.wait(lambda f: f[0] == 0x83 and f[18:] == b"Til dig, mens du var vaek", secs=20)
    ok("B online igen: beskeden hentet fra postkassen")
    # B's session overlevede genstarten (sessioner.bin), så posten var FS.
    with open(os.path.join(TMP, "b", "log.txt"), encoding="utf-8", errors="replace") as f:
        lines = [l for l in f if "besked fra " + addr_a.hex()[:8] in l]
    assert "forward secrecy)" in lines[-1], lines[-1]
    ok("post fra postkassen åbnet med den gemte forward secrecy-session")
    b.wait(lambda f: f[0] == 0x88 and f[1:17] == pj[1:17] and f[17:] == "Hytten æ".encode())
    ok("B husker den private kanal efter genstart")

    # 6b. Filpostkasse: B går offline, A sender en fil på 20 KB, den lægges i
    #     A's filpostkasse og leveres over TCP, når B kommer tilbage.
    def file_limit(c, dest):
        c.events.clear()
        c.send(frame(0x14, dest))
        f = c.wait(lambda f: f[0] == 0x96 and f[1:17] == dest)
        return struct.unpack("<I", f[17:21])[0], f[21], struct.unpack("<I", f[22:26])[0]
    def send_file(c, req, dest, data, name=b"post.webp"):
        c.send(frame(0x06, struct.pack("<H", req), dest, struct.pack("<I", len(data)), bytes([10]), b"image/webp", name))
        for off in range(0, len(data), 240):
            c.send(frame(0x07, struct.pack("<H", req), struct.pack("<I", off), data[off:off + 240]))
        c.send(frame(0x08, struct.pack("<H", req)))
    stop("b"); time.sleep(1)
    mx, flags, _ = file_limit(a, addr_b)
    assert mx == 1 << 20 and flags & 2, (mx, flags)
    ok("B offline: GetFileLimit siger postkasse, op til 1 MB")
    pdata = os.urandom(20_000)
    send_file(a, 30, addr_b, pdata)
    fid = a.wait(lambda f: f[0] == 0x8d and f[1:3] == struct.pack("<H", 30))[3:11]
    a.wait(lambda f: f[0] == 0x8c and f[1:9] == fid and f[9] == 1 and f[11:] == b"lagt i postkasse", secs=20)
    ok("fil på 20 KB lagt i A's filpostkasse")
    start("b", 8182, ["--listen", "off", "--peer", "127.0.0.1:4381", "--postkasse", addr_p.hex()])
    b = Client(8182)
    b.wait(lambda f: f[0] == 0x8c and f[1:9] == fid, secs=40)
    done = [f for f in b.events if f[0] == 0x8c and f[1:9] == fid][0]
    assert done[9] == 1, "fil fra postkassen fejlede: " + done[11:].decode()
    got = b"".join(f[13:] for f in sorted((f for f in b.events if f[0] == 0x8a and f[1:9] == fid), key=lambda f: struct.unpack("<I", f[9:13])[0]))
    assert hashlib.sha256(got).digest() == hashlib.sha256(pdata).digest()
    a.wait(lambda f: f[0] == 0x8c and f[1:9] == fid and f[11:] == b"leveret fra postkasse", secs=20)
    ok("B online igen: filen leveret fra filpostkassen og verificeret")
    mx, flags, _ = file_limit(a, addr_b)
    assert mx == 1 << 20 and flags == 0, (mx, flags)
    ok("B online over TCP: op til 1 MB, ikke over LoRa")

    # 6c. Over grænsen: uden filpostkasse og uden viden om ruten regnes den
    #     som LoRa (konservativt): 8 KB, og en fil på 20 KB afvises.
    start("c", 8184, ["--listen", "off", "--peer", "127.0.0.1:4381", "--filpostkasse-mb", "0"])
    c = Client(8184); c.info()
    ukendt = os.urandom(16)
    mx, flags, air = file_limit(c, ukendt)
    assert mx == 8192 and flags == 1 and air > 0, (mx, flags, air)
    ok(f"ukendt rute uden filpostkasse: 8 KB over LoRa (ca. {air} s sendetid)")
    c.send(frame(0x06, struct.pack("<H", 31), ukendt, struct.pack("<I", 20_000), bytes([10]), b"image/webp", b"stor.webp"))
    e = c.wait(lambda f: f[0] == 0x8f and f[1:3] == struct.pack("<H", 31))
    assert e[3] == 7 and "LoRa" in e[4:].decode(), e
    stop("c")
    ok("fil på 20 KB over LoRa afvist: " + e[4:].decode())

    # 7. Forkert origin
    try:
        websocket.create_connection("ws://127.0.0.1:8181/ws", origin="http://ond.example", timeout=3)
        raise AssertionError("forkert origin blev accepteret")
    except websocket.WebSocketBadStatusException:
        ok("WebSocket fra fremmed side afvist")

    # 8. Regioner over backbone: H (dk + nordjylland) med to peers,
    #    N (nordjylland) og K (dk). Nordjylland-beskeder når kun N.
    start("h", 8190, ["--listen", "127.0.0.1:4390", "--region", "dk", "--region", "nordjylland"])
    start("n", 8191, ["--listen", "off", "--peer", "127.0.0.1:4390", "--region", "nordjylland"])
    start("k", 8192, ["--listen", "off", "--peer", "127.0.0.1:4390", "--region", "dk"])
    h, n, k = Client(8190), Client(8191), Client(8192)
    for c in (h, n, k): c.info()
    h.send(frame(0x12))
    r = h.wait(lambda f: f[0] == 0x95)
    assert r[1] == 0 and r[2:] == b"dk,nordjylland", r
    ok("regioner sat med --region og hentet med GetRegions")
    time.sleep(1.5)  # regionshilsner udveksles
    for c in (n, k): c.send(frame(0x04, b"#tur"))
    h.send(frame(0x04, b"#tur@nordjylland"))
    j = h.wait(lambda f: f[0] == 0x88 and f[17:] == b"#tur@nordjylland")
    h.send(frame(0x03, struct.pack("<H", 20), j[1:17], b"Kun Nordjylland"))
    n.wait(lambda f: f[0] == 0x84 and f[34:] == b"Kun Nordjylland")
    k.pump(2)
    assert not any(f[0] == 0x84 and f[34:] == b"Kun Nordjylland" for f in k.events), "K fik en nordjysk besked"
    ok("#tur@nordjylland når N, men ikke K")
    h.send(frame(0x04, b"#tur@dk"))
    h.wait(lambda f: f[0] == 0x88 and f[17:] == b"#tur@dk")
    h.send(frame(0x03, struct.pack("<H", 21), j[1:17], b"Hele Danmark"))
    k.wait(lambda f: f[0] == 0x84 and f[34:] == b"Hele Danmark")
    n.pump(2)
    assert not any(f[0] == 0x84 and f[34:] == b"Hele Danmark" for f in n.events), "N (kun nordjylland) fik en dk-besked"
    ok("#tur@dk når K, men ikke N (der kun betjener nordjylland)")
    # Tom liste og strict via companion-kommandoen.
    n.send(frame(0x11, bytes([1]), b""))
    r = n.wait(lambda f: f[0] == 0x95 and f[1] == 1)
    ok("SetRegions (strict, tom liste) besvaret")

    # Firmwareopdatering (0x1b OtaStatus): maarse-host opdateres ikke over
    # luften og svarer "ikke understøttet" (tilstand 5) i OtaState (0x9a).
    n.send(frame(0x1b))
    n.wait(lambda f: f[0] == 0x9a and f[3] == 5)
    ok("OtaStatus besvaret (maarse-host: ikke understøttet)")
    print("ALLE TESTS OK")
finally:
    for n in list(procs): 
        try: stop(n)
        except Exception: pass
