"""Records the spectrum on a Må:rse radio over USB and saves the file the radio sends back.

Usage: python tools/spectrum_rec_test.py COM6 [duration_s] [interval_tenths] [out.msr]

The radio records on its own (command 0x3e), then hands the file to this client as an incoming file, the same way
a file from another node arrives. It cannot hear LoRa while a sweep runs (about 100 ms each).
"""
import struct, sys, time, serial

port = sys.argv[1]
duration = int(sys.argv[2]) if len(sys.argv) > 2 else 20
interval = int(sys.argv[3]) if len(sys.argv) > 3 else 5
out = sys.argv[4] if len(sys.argv) > 4 else "spectrum.msr"
STATES = {0: "idle", 1: "recording", 2: "ready", 3: "sending", 4: "failed"}

# dtr=True, rts=False: the default open (DTR and RTS together) resets an ESP32-S3.
s = serial.Serial()
s.port, s.baudrate, s.timeout = port, 115200, 0.1
s.dtr, s.rts = True, False
s.open()
buf = b""


def send(typ, body=b""):
    s.write(struct.pack("<HB", len(body) + 1, typ) + body)


def frames(secs):
    global buf
    end = time.time() + secs
    while time.time() < end:
        buf += s.read(512)
        while len(buf) >= 2:
            n = struct.unpack_from("<H", buf)[0]
            if n == 0 or n > 302:
                buf = buf[1:]
                continue
            if len(buf) < 2 + n:
                break
            f, buf = buf[2:2 + n], buf[2 + n:]
            yield f[0], f[1:]


send(0x01)  # GetInfo
for t, b in frames(1.5):
    if t == 0x81:
        print("radio:", b[:16].hex(), "version", b[17])
        break

send(0x3F)  # stop anything that runs
list(frames(0.3))
send(0x3E, struct.pack("<IHHBB", 863000, 110, duration, interval, 6))
print(f"asked for {duration} s, one sweep every {interval / 10:g} s")
t0 = time.time()
data = {}
done = None
state = None
while time.time() - t0 < duration + 60 and done is None:
    for t, b in frames(1.0):
        if t == 0xB9:
            st, el, du, sw, nb = struct.unpack("<BHHHH", b[:9])
            if (st, el // 5) != state:
                print(f"  {STATES.get(st, st)}: {el}/{du} s, {sw} sweeps, file {nb} bytes")
                state = (st, el // 5)
        elif t == 0x89:
            print("  incoming file:", b[8 + 16 + 4 + 0:][:0] or "")
        elif t == 0x8A:
            fid, off = struct.unpack_from("<QI", b)
            data[off] = b[12:]
        elif t == 0x8C:
            done = b
        elif t == 0x8E:
            print("  log:", b.decode(errors="replace"))
        elif t == 0x8F:
            print("  error:", b[3:].decode(errors="replace"))
    if time.time() - t0 > 3 and int(time.time() - t0) % 5 == 0:
        send(0x43)  # status
if done is None:
    print("no file arrived")
    sys.exit(1)
blob = b"".join(data[k] for k in sorted(data))
open(out, "wb").write(blob)
print(f"file received: {len(blob)} bytes -> {out}")
