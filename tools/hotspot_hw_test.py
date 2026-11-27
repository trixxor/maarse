"""Hardware check of the Wi-Fi hotspot on an ESP32 node (Heltec V4).

Usage:
  python tools/hotspot_hw_test.py COM6 on "Maarse repeater" "passphrase of 8+ chars"
  python tools/hotspot_hw_test.py COM6 status
  python tools/hotspot_hw_test.py COM6 off

`on` saves the hotspot settings (SetWifi, flags enabled + hotspot); the node restarts and opens the network about
10 seconds later. `status` asks for the Wi-Fi state (GetWifi): state 4 = network open, 5 = a phone is connected,
and the address should be 192.168.4.1. `off` turns Wi-Fi off again (Bluetooth comes back after the restart).
"""
import struct
import sys
import time

import serial


def frame(typ, body=b""):
    return struct.pack("<HB", len(body) + 1, typ) + body


def read_frames(s, secs):
    buf = b""
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


STATES = {0: "off", 1: "joining", 2: "online", 3: "linked", 4: "hotspot open", 5: "hotspot, phone connected",
          10: "skipped after crash", 11: "bad settings", 12: "driver failed"}


def open_port(port):
    s = serial.Serial()
    s.port = port
    s.baudrate = 115200
    s.timeout = 0.1
    s.dtr = False  # opening with DTR/RTS asserted resets the board
    s.rts = False
    s.open()
    return s


def status(s):
    s.write(frame(0x29))
    for typ, b in read_frames(s, 3):
        if typ == 0xA6:
            flags, state = b[0], b[1]
            ip = ".".join(str(x) for x in b[2:6])
            n = b[6]
            ssid = b[7:7 + n].decode(errors="replace")
            print(f"enabled={flags & 1} modem={(flags >> 1) & 1} hotspot={(flags >> 2) & 1} state={state} ({STATES.get(state, '?')}) ip={ip} ssid={ssid!r}")
            return state
    print("no answer to GetWifi")
    return None


def main():
    port, cmd = sys.argv[1], sys.argv[2]
    s = open_port(port)
    if cmd == "status":
        return 0 if status(s) in (4, 5) else 1
    if cmd == "on":
        ssid, password = sys.argv[3].encode(), (sys.argv[4].encode() if len(sys.argv) > 4 else b"")
        s.write(frame(0x28, bytes([1 | 4, len(ssid)]) + ssid + bytes([len(password)]) + password))
        for typ, b in read_frames(s, 3):
            if typ == 0x8E:
                print("node:", b.decode(errors="replace"))
        print("saved; the node restarts and opens the network in about 10 seconds")
        return 0
    if cmd == "off":
        s.write(frame(0x28, bytes([0, 0, 0])))
        for typ, b in read_frames(s, 3):
            if typ == 0x8E:
                print("node:", b.decode(errors="replace"))
        return 0
    print(__doc__)
    return 2


sys.exit(main())
