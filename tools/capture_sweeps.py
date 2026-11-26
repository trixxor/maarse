"""Captures raw spectrum sweeps from a Må:rse radio over USB, for trying out recording settings on real data.

Usage: python tools/capture_sweeps.py COM6 [seconds] [out.bin]

Writes one record per sweep: time since the start in ms (u32, little endian) and 64 bytes (-dBm x 2, 255 = not
measured), the same layout as maarse-core/tests/data/sweeprec-golden-sweeps.bin. The radio ends a run of sweeps after
2 minutes by itself, so the run is stopped and started again every 100 s; the radio hears no messages meanwhile.
"""
import struct, sys, time, serial

port = sys.argv[1]
secs = float(sys.argv[2]) if len(sys.argv) > 2 else 300
out = sys.argv[3] if len(sys.argv) > 3 else "sweeps.bin"
s = serial.Serial()
s.port, s.baudrate, s.timeout = port, 115200, 0.05
s.dtr, s.rts = True, False  # the default open (DTR and RTS together) resets an ESP32-S3
s.open()
buf = b""


def send(typ, body=b""):
    s.write(struct.pack("<HB", len(body) + 1, typ) + body)


rows = []
t0 = time.time()
send(0x01)
time.sleep(1)
send(0x3D, struct.pack("<IHBB", 863000, 110, 64, 0))  # stop anything that runs
time.sleep(0.5)
s.reset_input_buffer()
send(0x3D, struct.pack("<IHBB", 863000, 110, 64, 255))
t0 = time.time()
last_restart = t0
while time.time() - t0 < secs:
    buf += s.read(2048)
    while len(buf) >= 2:
        n = struct.unpack_from("<H", buf)[0]
        if n == 0 or n > 302:
            buf = buf[1:]
            continue
        if len(buf) < 2 + n:
            break
        f, buf = buf[2:2 + n], buf[2 + n:]
        if f[0] == 0xBF and len(f) >= 9 + 64:
            rows.append((int((time.time() - t0) * 1000), f[9:9 + 64]))
    if time.time() - last_restart > 100:
        send(0x3D, struct.pack("<IHBB", 0, 0, 0, 0))
        time.sleep(0.3)
        send(0x3D, struct.pack("<IHBB", 863000, 110, 64, 255))
        last_restart = time.time()
send(0x3D, struct.pack("<IHBB", 0, 0, 0, 0))
with open(out, "wb") as fh:
    for t, r in rows:
        fh.write(struct.pack("<I", t) + r)
print(f"{len(rows)} sweeps in {time.time() - t0:.0f} s ({len(rows) / (time.time() - t0):.1f} per second) -> {out}")
