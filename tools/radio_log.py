"""Prints the log, error and recording-state events a Må:rse radio sends over USB, for a number of seconds.

Usage: python tools/radio_log.py COM6 [seconds]
"""
import struct, sys, time, serial
sys.stdout.reconfigure(encoding="utf-8", errors="replace")

s = serial.Serial()
s.port, s.baudrate, s.timeout = sys.argv[1], 115200, 0.1
s.dtr, s.rts = True, False  # the default open (DTR and RTS together) resets an ESP32-S3
s.open()
buf = b""
junk = b""
t0 = time.time()
secs = float(sys.argv[2]) if len(sys.argv) > 2 else 60
s.write(struct.pack("<HB", 1, 0x01))
while time.time() - t0 < secs:
    buf += s.read(512)
    while len(buf) >= 2:
        n = struct.unpack_from("<H", buf)[0]
        if n == 0 or n > 302:
            # Not a frame: text the firmware prints on its own (a start-up banner, a panic).
            junk += buf[:1]
            buf = buf[1:]
            if junk.endswith(b"\n") and junk.strip():
                print(f"{time.time() - t0:6.1f}s text:", junk.decode("latin-1").strip()[:200], flush=True)
                junk = b""
            continue
        if len(buf) < 2 + n:
            break
        f, buf = buf[2:2 + n], buf[2 + n:]
        t, b = f[0], f[1:]
        stamp = f"{time.time() - t0:6.1f}s"
        if t == 0x8E:
            print(stamp, "log:", b.decode(errors="replace"), flush=True)
        elif t == 0x8F:
            print(stamp, "error:", b[3:].decode(errors="replace"), flush=True)
        elif t == 0xB9:
            print(stamp, "recording state:", struct.unpack("<BHHHH", b[:9]), flush=True)
        elif t == 0x83:
            print(stamp, "direct message received", flush=True)
