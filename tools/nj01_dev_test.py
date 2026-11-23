# Dev test over the real backbone: a throw-away host joins nj01 and writes to one
# account (a person address) and into #public, so a real phone can show the
# notification (and its sound). Nothing is sent over MeshCore.
#   pip install websocket-client
#   python tools/nj01_dev_test.py <person address, 32 hex> "<name to mention>" [target/debug]
import os, struct, subprocess, sys, tempfile, time
import websocket

PERSON = bytes.fromhex(sys.argv[1])
NAME = sys.argv[2]
BIN = sys.argv[3] if len(sys.argv) > 3 else "target/debug"
HOST = os.path.abspath(os.path.join(BIN, "maarse-host" + (".exe" if os.name == "nt" else "")))
TMP = tempfile.mkdtemp(prefix="maarse-nj01-test-")
PUBLIC = bytes.fromhex("63369c1e62dd4450a862c807bcb4b826")


def frame(kind, *parts):
    body = bytes([kind]) + b"".join(parts)
    return struct.pack("<H", len(body)) + body


p = subprocess.Popen([HOST, "--data", TMP, "--http", "127.0.0.1:8197", "--no-browser", "--no-history", "--listen", "off", "--public"],
                     stdout=open(os.path.join(TMP, "log.txt"), "w"), stderr=subprocess.STDOUT)
try:
    for _ in range(50):
        try:
            ws = websocket.create_connection("ws://127.0.0.1:8197/ws", origin="http://127.0.0.1:8197", timeout=5)
            break
        except OSError:
            time.sleep(0.2)
    ws.settimeout(0.2)
    ws.send_binary(frame(0x01))
    events = []

    def pump(secs):
        end, buf = time.time() + secs, b""
        while time.time() < end:
            try:
                buf += ws.recv()
            except websocket.WebSocketTimeoutException:
                continue
            while len(buf) >= 2:
                n = struct.unpack("<H", buf[:2])[0]
                if len(buf) < 2 + n:
                    break
                events.append(buf[2:2 + n]); buf = buf[2 + n:]

    print("joining nj01 and waiting 40 s to hear the account's announces...")
    pump(25)
    ws.send_binary(frame(0x05)); pump(2)
    ws.send_binary(frame(0x04, b"#public")); pump(1)
    ws.send_binary(frame(0x02, struct.pack("<H", 1), PERSON, "Dev test: direct message to the account".encode())); pump(15)
    ws.send_binary(frame(0x03, struct.pack("<H", 2), PUBLIC, f"@{NAME} dev test mention".encode())); pump(10)
    kinds = {}
    for e in events:
        kinds[e[0]] = kinds.get(e[0], 0) + 1
    print("event kinds seen (hex: count):", {hex(k): v for k, v in sorted(kinds.items())})
    print("Sent:", any(e[0] == 0x82 for e in events), " Acked:", any(e[0] == 0x85 for e in events), " Error:", [e[1:] for e in events if e[0] == 0x8f][:2])
    print(open(os.path.join(TMP, "log.txt"), errors="replace").read()[-1500:])
finally:
    p.terminate()
