"""Range test from a PC: one node as beacon, one as receiver (or either alone).

Drives the firmware's range test over USB (commands 0x2a/0x2b, events
0xa8-0xab, see maarse-core/src/rangetest.rs and docs/FIELD-TEST.md), prints
every beacon received, writes them to a CSV file and prints the per-SF results
and power estimates at the end. Useful as a bench test with both boards on one
PC, or with a laptop as the receiver in the car (no GPS here: use the web
client's Range test page on a phone for positions).

Examples:
    python tools/range_test.py --beacon COM6 --receiver COM7 --minutes 5
    python tools/range_test.py --beacon COM6 --receiver COM7 --cycle 7,8,9,10,11,12 --per-step 3 --interval 5
    python tools/range_test.py --receiver COM7 --minutes 60 --csv drive.csv --leave-running
"""
import argparse, csv, struct, sys, time
import serial

MODES = {0: "off", 1: "beacon", 2: "receiver"}


class Node:
    def __init__(self, port, role):
        self.port, self.role = port, role
        s = serial.Serial()
        s.port, s.baudrate, s.timeout = port, 115200, 0.05
        # The Heltec V4 resets when the port is opened with the default lines.
        s.dtr, s.rts = True, False
        s.open()
        self.s, self.buf = s, b""
        self.state = self.power = None

    def send(self, typ, body=b""):
        self.s.write(struct.pack("<HB", len(body) + 1, typ) + body)

    def frames(self):
        self.buf += self.s.read(4096)
        while len(self.buf) >= 2:
            n = struct.unpack_from("<H", self.buf)[0]
            if n == 0 or n > 302:
                self.buf = self.buf[1:]
                continue
            if len(self.buf) < 2 + n:
                break
            f, self.buf = self.buf[2:2 + n], self.buf[2 + n:]
            yield f[0], f[1:]


def config(mode, a):
    mask = sum(1 << (sf - 7) for sf in a.cycle) if a.cycle else 0
    flags = (1 if a.persist else 0) | (2 if a.lat is not None else 0)
    lat = round((a.lat or 0) * 1e7)
    lon = round((a.lon or 0) * 1e7)
    return struct.pack("<BBHBBBii", mode, flags, a.interval, mask, a.per_step, a.pad, lat, lon)


def parse_state(b):
    mode, flags, interval, mask, per_step, pad, lat, lon = struct.unpack_from("<BBHBBBii", b)
    sf, synced = b[15], b[16] & 1
    count, missed, seq = struct.unpack_from("<III", b, 17)
    per_sf = [struct.unpack_from("<BHHhhhb", b, o) for o in range(29, len(b) - 11, 12)]
    return dict(mode=mode, interval=interval, mask=mask, per_step=per_step, sf=sf, synced=synced,
                count=count, missed=missed, seq=seq, per_sf=per_sf)


def parse_rx(b):
    up, sender, seq, sf, rssi, snr, ln, lost, skipped, txp, mv = struct.unpack_from("<I4sIBhbBHHbH", b)
    pos = struct.unpack_from("<ii", b, 24) if len(b) >= 32 else None
    return dict(uptime_ms=up, beacon=sender.hex(), seq=seq, sf=sf, rssi=rssi, snr=snr, length=ln, lost=lost,
                skipped=skipped, tx_dbm=txp, beacon_mv=mv,
                beacon_lat=pos[0] / 1e7 if pos else "", beacon_lon=pos[1] / 1e7 if pos else "")


def parse_tx(b):
    up, seq, sf, slot, interval, air, charge, skipped = struct.unpack_from("<IIBBHHIH", b)
    return dict(uptime_ms=up, seq=seq, sf=sf, slot=slot, interval=interval, air_ms=air, charge=charge, skipped=skipped)


def parse_power(b):
    up, mv, ext, mode, txp, base, rx, tx, pkts = struct.unpack_from("<IHbBbHHHI", b)
    phases = [struct.unpack_from("<III", b, 19 + 12 * i) for i in range(3)]
    return dict(uptime=up, mv=mv, ext=ext, mode=mode, tx_dbm=txp, base=base / 10, rx=rx / 10, tx=tx / 10,
                tx_packets=pkts, phases=phases)


def show_power(n):
    p = n.power
    if not p:
        return
    print(f"  [{n.port}] power estimate (base {p['base']} mA, receive {p['rx']} mA, transmit {p['tx']} mA at "
          f"{p['tx_dbm']} dBm; battery {p['mv']} mV, USB {p['ext']}):")
    for name, (secs, tx_ms, mas) in zip(("off", "beacon", "receiver"), p["phases"]):
        if secs:
            print(f"      {name:9} {secs:6} s, TX {tx_ms / 1000:7.1f} s ({100 * tx_ms / 1000 / secs:5.2f} %), "
                  f"{mas / 3600:7.2f} mAh, average {mas / secs:6.1f} mA")


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--beacon", help="serial port of the beacon node")
    ap.add_argument("--receiver", help="serial port of the receiver node")
    ap.add_argument("--minutes", type=float, default=5)
    ap.add_argument("--interval", type=int, default=10, help="seconds between beacons (2-255)")
    ap.add_argument("--cycle", type=lambda s: [int(x) for x in s.split(",") if x], default=[],
                    help="SFs to cycle through, e.g. 7,8,9,10,11,12 (default: stay on the radio's SF)")
    ap.add_argument("--per-step", type=int, default=5, help="beacons per SF when cycling")
    ap.add_argument("--pad", type=int, default=0, help="extra bytes per beacon")
    ap.add_argument("--lat", type=float, help="beacon position to send in every beacon")
    ap.add_argument("--lon", type=float)
    ap.add_argument("--persist", action="store_true", help="keep the setting in flash")
    ap.add_argument("--csv", default="range-received.csv")
    ap.add_argument("--leave-running", action="store_true", help="do not stop the test at the end")
    a = ap.parse_args()
    if not a.beacon and not a.receiver:
        ap.error("give --beacon and/or --receiver")

    nodes = []
    if a.receiver:
        nodes.append(Node(a.receiver, "receiver"))
    if a.beacon:
        nodes.append(Node(a.beacon, "beacon"))
    time.sleep(0.5)
    for n in nodes:
        n.send(0x0f, struct.pack("<I", int(time.time())))
        n.send(0x0b)  # GetRadio
    for n in nodes:  # receiver first, so it listens before the first beacon
        n.send(0x2a, config(1 if n.role == "beacon" else 2, a))
        time.sleep(0.3)

    out = open(a.csv, "w", newline="")
    w = None
    end = time.time() + a.minutes * 60
    last_poll = time.time()
    rx_count = 0
    try:
        while time.time() < end:
            for n in nodes:
                for typ, b in n.frames():
                    if typ == 0x92:
                        f, bw, sf, cr, p = struct.unpack_from("<IIBBb", b)
                        duty = struct.unpack_from("<H", b, 11)[0]
                        print(f"  [{n.port}] radio: {f / 1e6:.3f} MHz, BW {bw / 1000:g} kHz, SF{sf}, CR4/{cr}, {p} dBm, duty {duty / 10:g} %")
                    elif typ == 0xa8:
                        n.state = parse_state(b)
                        s = n.state
                        print(f"  [{n.port}] state: {MODES.get(s['mode'])} SF{s['sf']} synced={s['synced']} "
                              f"count={s['count']} missed={s['missed']} seq={s['seq']}")
                    elif typ == 0xa9:
                        r = parse_rx(b)
                        r["time_iso"] = time.strftime("%Y-%m-%dT%H:%M:%S")
                        rx_count += 1
                        print(f"  [{n.port}] RX #{r['seq']:5} SF{r['sf']:2} {r['rssi']:5} dBm SNR {r['snr']:4} dB"
                              f"{'  lost ' + str(r['lost']) if r['lost'] else ''}")
                        if w is None:
                            w = csv.DictWriter(out, fieldnames=["time_iso"] + [k for k in r if k != "time_iso"])
                            w.writeheader()
                        w.writerow(r)
                        out.flush()
                    elif typ == 0xaa:
                        t = parse_tx(b)
                        print(f"  [{n.port}] TX #{t['seq']:5} SF{t['sf']:2} {t['air_ms']:5} ms on air, every {t['interval']} s, "
                              f"~{t['charge'] / 3600:.1f} uAh (estimate){'  skipped so far ' + str(t['skipped']) if t['skipped'] else ''}")
                    elif typ == 0xab:
                        n.power = parse_power(b)
                    elif typ == 0x8e:
                        print(f"  [{n.port}] log: {b.decode(errors='replace')}")
                    elif typ == 0x8f:
                        print(f"  [{n.port}] error: {b[3:].decode(errors='replace')}")
            if time.time() - last_poll > 30:
                last_poll = time.time()
                for n in nodes:
                    n.send(0x2b)
            time.sleep(0.02)
    except KeyboardInterrupt:
        print("stopped")
    for n in nodes:
        n.send(0x2b)
    t0 = time.time()
    while time.time() - t0 < 2:
        for n in nodes:
            for typ, b in n.frames():
                if typ == 0xa8:
                    n.state = parse_state(b)
                elif typ == 0xab:
                    n.power = parse_power(b)
        time.sleep(0.02)
    if not a.leave_running:
        for n in nodes:
            n.send(0x2a, struct.pack("<BBHBBBii", 0, 0, 10, 0, 1, 0, 0, 0))
    out.close()

    print(f"\nResults ({rx_count} beacons logged to {a.csv}):")
    for n in nodes:
        s = n.state
        if not s:
            continue
        print(f"  [{n.port}] {n.role}: count={s['count']} missed={s['missed']} last/next seq={s['seq']}")
        if s["per_sf"]:
            print("      SF  rx  lost  loss%  RSSI avg/min  SNR avg/min")
            for sf, rx, lost, ravg, rmin, savg4, smin in s["per_sf"]:
                tot = rx + lost
                print(f"      {sf:2} {rx:4} {lost:5} {100 * lost / tot if tot else 0:6.1f}  {ravg:5}/{rmin:5}    {savg4 / 4:5.1f}/{smin:4}")
        show_power(n)


if __name__ == "__main__":
    sys.exit(main())
