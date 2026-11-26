"""Prints a few counters a Må:rse radio reports over USB (stats keys): uptime, known nodes, routes, LoRa packets in and out.

Usage: python tools/radio_counters.py COM6
"""
import struct, sys, time, serial

NAMES = {1: "uptime_s", 2: "known_nodes", 3: "routes", 7: "role", 10: "rx_packets", 11: "rx_invalid", 15: "delivered",
         30: "radio_rx_ok", 31: "radio_rx_err", 32: "radio_tx_ok", 33: "radio_tx_err", 34: "last_rssi", 35: "last_snr", 44: "tx_queue"}
s = serial.Serial()
s.port, s.baudrate, s.timeout = sys.argv[1], 115200, 0.1
s.dtr, s.rts = True, False  # the default open (DTR and RTS together) resets an ESP32-S3
s.open()
buf = b""
s.write(struct.pack("<HB", 1, 0x09))
t0 = time.time()
vals = {}
while time.time() - t0 < 3:
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
print({NAMES[k]: vals.get(k) for k in NAMES})
