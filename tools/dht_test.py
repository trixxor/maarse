# Test of the backbone DHT: A and C each hang off their own backbone host (B2, B3),
# which do NOT have a packet connection to each other — only the DHT seed B1.
# A must still be able to write to C: lookup in the DHT -> direct connection
# to C's gateway -> ordinary routing.
#   python tools/dht_test.py <path to maarse-host>
import os, struct, subprocess, sys, tempfile, time
import websocket

HOST = sys.argv[1] if len(sys.argv) > 1 else "target/release/maarse-host"
procs = {}
TMP = tempfile.mkdtemp(prefix="maarse-dht-")

def start(name, http, extra):
    d = os.path.join(TMP, name); os.makedirs(d, exist_ok=True)
    procs[name] = subprocess.Popen([os.path.abspath(HOST), "--no-browser", "--data", d, "--http", f"127.0.0.1:{http}"] + extra,
                                   stdout=open(os.path.join(d, "log.txt"), "a"), stderr=subprocess.STDOUT)
    time.sleep(0.5)

def frame(kind, *parts):
    body = bytes([kind]) + b"".join(parts)
    return struct.pack("<H", len(body)) + body

class Client:
    def __init__(self, port):
        self.ws = websocket.create_connection(f"ws://127.0.0.1:{port}/ws", origin=f"http://127.0.0.1:{port}", timeout=5)
        self.buf = b""; self.events = []
    def send(self, b): self.ws.send_binary(b)
    def pump(self, secs):
        end = time.time() + secs; self.ws.settimeout(0.1)
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
    def stats(self):
        self.events.clear(); self.send(frame(0x09))
        f = self.wait(lambda f: f[0] == 0x91)
        return {f[i]: struct.unpack("<i", f[i+1:i+5])[0] for i in range(1, len(f) - 4, 5)}

try:
    start("b1", 8190, ["--listen", "127.0.0.1:4390", "--public-addr", "127.0.0.1:4390"])
    start("b2", 8191, ["--listen", "127.0.0.1:4391", "--public-addr", "127.0.0.1:4391", "--dht", "127.0.0.1:4390"])
    start("b3", 8192, ["--listen", "127.0.0.1:4392", "--public-addr", "127.0.0.1:4392", "--dht", "127.0.0.1:4390"])
    time.sleep(2)
    start("a", 8193, ["--listen", "off", "--peer", "127.0.0.1:4391"])
    start("c", 8194, ["--listen", "off", "--peer", "127.0.0.1:4392"])
    a, c = Client(8193), Client(8194)
    addr_a, addr_c = a.info(), c.info()
    time.sleep(8)  # B3 publishes C's address, B2 publishes A's
    b1 = Client(8190)
    st = b1.stats()
    print(f"  B1: {st.get(72)} contacts, {st.get(73)} records")
    assert st.get(73, 0) >= 2, "the records have not been published"
    assert not any(f[0] == 0x87 and f[1:17] == addr_c for f in a.events), "A must not have heard C directly"

    a.send(frame(0x02, struct.pack("<H", 7), addr_c, b"Hi C via DHT"))
    c.wait(lambda f: f[0] == 0x83 and f[18:] == b"Hi C via DHT", secs=40)
    print("  OK  A -> C found via DHT and delivered")
    a.wait(lambda f: f[0] == 0x85, secs=20)
    print("  OK  acknowledgement back")
    c.send(frame(0x02, struct.pack("<H", 8), addr_a, b"Hi A"))
    a.wait(lambda f: f[0] == 0x83 and f[18:] == b"Hi A", secs=40)
    print("  OK  C -> A")
    print("DHT test passed")
finally:
    for p in procs.values(): p.terminate()
    for p in procs.values(): p.wait()
