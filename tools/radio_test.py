"""Test of two Må:rse radios over USB/serial and LoRa.

Usage: python tools/radio_test.py COM6 COM5
"""
import struct, sys, time, serial

NAMES = {0x81: "Info", 0x82: "Sent", 0x83: "DirectReceived", 0x84: "ChannelReceived",
         0x85: "Acked", 0x86: "RouteLearned", 0x87: "Heard", 0x88: "ChannelJoined",
         0x8e: "Log", 0x8f: "Error", 0x90: "Deposited", 0x91: "Stats", 0x92: "Radio"}
STATS = {1: "uptime_s", 2: "known_nodes", 3: "routes", 7: "role", 10: "rx_packets",
         11: "rx_invalid", 13: "forwarded", 15: "delivered", 16: "acked",
         30: "radio_rx_ok", 31: "radio_rx_err", 32: "radio_tx_ok", 33: "radio_tx_err",
         34: "last_rssi", 35: "last_snr", 38: "cad_busy", 40: "airtime_ms",
         45: "freq_khz", 46: "sf", 47: "bw_khz", 48: "dbm", 49: "battery_mv", 44: "tx_queue", 53: "radio_step", 54: "radio_loops", 55: "diag_syncword", 56: "diag_status", 57: "diag_err", 58: "diag_busy"}


class Node:
    def __init__(self, port):
        self.port = port
        self.s = serial.Serial(port, 115200, timeout=0.1)
        self.buf = b""
        self.addr = None

    def send(self, typ, body=b""):
        self.s.write(struct.pack("<HB", len(body) + 1, typ) + body)

    def events(self, secs):
        end = time.time() + secs
        while time.time() < end:
            self.buf += self.s.read(512)
            while len(self.buf) >= 2:
                n = struct.unpack_from("<H", self.buf)[0]
                if n == 0 or n > 302:  # out of sync: drop a byte
                    self.buf = self.buf[1:]
                    continue
                if len(self.buf) < 2 + n:
                    break
                f, self.buf = self.buf[2:2 + n], self.buf[2 + n:]
                yield f[0], f[1:]

    def log(self, typ, b):
        name = NAMES.get(typ, hex(typ))
        if typ == 0x81:
            self.addr = b[:16]
            mf = struct.unpack_from("<I", b, 19)[0]
            txt = f"address={b[:16].hex()} repeater={b[16]} version={b[17]} max_text={b[18]} max_file={mf}"
        elif typ == 0x92:
            f, bw, sf, cr, p = struct.unpack_from("<IIBBb", b)
            duty, pre = struct.unpack_from("<HH", b, 11)
            txt = f"{f/1e6:.3f} MHz, BW {bw/1000:g} kHz, SF{sf}, CR4/{cr}, {p} dBm, duty {duty/10:g} %, preamble {pre}"
        elif typ == 0x91:
            items = [struct.unpack_from("<Bi", b, i) for i in range(0, len(b) - 4, 5)]
            txt = ", ".join(f"{STATS.get(k, k)}={v}" for k, v in items)
        elif typ in (0x83, 0x84):
            o = 16 if typ == 0x83 else 32
            txt = f"from={b[o-16:o].hex()[:8]}… hops={b[o]} text={b[o+1:].decode(errors='replace')!r}"
        elif typ in (0x86, 0x87):
            txt = f"{b[:16].hex()[:8]}… hops={b[16]}"
        elif typ == 0x8e:
            txt = b.decode(errors="replace")
        elif typ == 0x8f:
            txt = f"req={struct.unpack_from('<H', b)[0]} code={b[2]} {b[3:].decode(errors='replace')}"
        else:
            txt = b.hex()
        print(f"  [{self.port}] {name}: {txt}")

    def pump(self, secs, want=None):
        got = []
        for typ, b in self.events(secs):
            self.log(typ, b)
            got.append((typ, b))
            if want is not None and typ == want:
                break
        return got


def pump_all(nodes, secs):
    end = time.time() + secs
    seen = {n.port: [] for n in nodes}
    while time.time() < end:
        for n in nodes:
            seen[n.port] += n.pump(0.2)
    return seen


def main():
    nodes = [Node(p) for p in sys.argv[1:3]]
    time.sleep(0.5)
    print("== Info and radio ==")
    for n in nodes:
        n.send(0x0f, struct.pack("<I", int(time.time())))
        n.send(0x01)
        n.send(0x0b)
        n.pump(2)
    if not all(n.addr for n in nodes):
        print("FAIL: did not get Info from both nodes"); return 1

    print("== Announce over LoRa ==")
    for n in nodes:
        n.send(0x05)
        pump_all(nodes, 6)

    a, b = nodes
    ok = True
    for src, dst, req in ((a, b, 1), (b, a, 2)):
        txt = f"hello from {src.port} {time.strftime('%H:%M:%S')}".encode()
        print(f"== Direct message {src.port} -> {dst.port} ==")
        src.send(0x02, struct.pack("<H", req) + dst.addr + txt)
        seen = pump_all(nodes, 25)
        got = any(t == 0x83 and txt in body for t, body in seen[dst.port])
        acked = any(t == 0x85 for t, _ in seen[src.port])
        print(f"  -> received: {'YES' if got else 'NO'}, acked: {'YES' if acked else 'NO'}")
        ok &= got and acked

    print("== Status ==")
    for n in nodes:
        n.send(0x09)
        n.pump(2, want=0x91)
    print("RESULT:", "OK" if ok else "FAIL")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
