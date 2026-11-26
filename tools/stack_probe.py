"""Prints the free stack, free heap and uptime a Må:rse radio reports (stats keys 87, 88, 89, 1) over USB.

Usage: python tools/stack_probe.py COM6
"""
import struct, sys, time, serial

s = serial.Serial()
s.port, s.baudrate, s.timeout = sys.argv[1], 115200, 0.1
s.dtr, s.rts = True, False  # the default open (DTR and RTS together) resets an ESP32-S3
s.open()
buf = b""
s.write(struct.pack("<HB", 1, 0x09))  # GetStats
t0 = time.time()
vals = {}
while time.time() - t0 < 4:
    buf += s.read(512)
    while len(buf) >= 2:
        n = struct.unpack_from("<H", buf)[0]
        if n == 0 or n > 302:
            buf = buf[1:]
            continue
        if len(buf) < 2 + n:
            break
        f, buf = buf[2:2 + n], buf[2 + n:]
        if f[0] == 0x91:
            b = f[1:]
            for i in range(0, len(b) - 4, 5):
                k, v = struct.unpack_from("<Bi", b, i)
                vals[k] = v
print({k: vals.get(k) for k in (1, 87, 88, 89)})
