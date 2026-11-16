# Test of a Wi-Fi LoRa modem at a fixed address (--modem-ip) with a modem key,
# and of the modem's counters reaching the server's telemetry over Wi-Fi.
# A fake modem listens; maarse-server connects to it.
#   python tools/wifi_modem_test.py [target/release]
import hashlib, http.cookiejar, json, os, socket, struct, subprocess, sys, tempfile, threading, time, urllib.request

BIN = sys.argv[1] if len(sys.argv) > 1 else "target/release"
EXE = ".exe" if os.name == "nt" else ""
SERVER = os.path.abspath(os.path.join(BIN, "maarse-server" + EXE))
TMP = tempfile.mkdtemp(prefix="maarse-wifimodem-test-")
PASS = "fixed address modem key"
KEY = hashlib.sha256(b"maarse/modem-key" + PASS.encode()).digest()
state = {"challenged": False, "stats_sent": 0}


def fake_modem(port):
    ls = socket.socket(); ls.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    ls.bind(("127.0.0.1", port)); ls.listen(1)
    c, _ = ls.accept()
    c.sendall(b"MM")
    c.settimeout(10)
    n = struct.unpack("<H", c.recv(2))[0]
    body = c.recv(n)
    if body.startswith(b"\xffMK"):
        state["challenged"] = True
        tag = hashlib.sha256(b"maarse/modem-auth" + KEY + body[3:]).digest()[:16]
        c.sendall(struct.pack("<H", 20) + b"\x7f\xffMR" + tag)
    # Event::Stats (0x91): key u8 | value u32 LE ...; as the firmware sends it, after 0x7E.
    stats = bytes([0x91]) + b"".join(bytes([k]) + struct.pack("<i", v) for k, v in [(1, 1234), (30, 42), (40, 5000), (41, 7), (49, 3990), (80, -98)])
    more = bytes([0x91]) + b"".join(bytes([k]) + struct.pack("<i", v) for k, v in [(93, 3), (97, 2)])
    for f in (stats, more):
        c.sendall(struct.pack("<H", len(f) + 1) + b"\x7e" + f)
        state["stats_sent"] += 1
    time.sleep(30)
    c.close()


threading.Thread(target=fake_modem, args=(4391,), daemon=True).start()
d = os.path.join(TMP, "server")
subprocess.run([SERVER, "--set-admin-password", "wifi-modem-test-1", "--data", d], check=True, capture_output=True)
p = subprocess.Popen([SERVER, "--data", d, "--listen", "127.0.0.1:4390", "--public-addr", "127.0.0.1:4390", "--http", "127.0.0.1:8490", "--modem-key", PASS, "--modem-ip", "127.0.0.1:4391"],
                     stdout=open(d + ".log", "w"), stderr=subprocess.STDOUT)
try:
    cj = http.cookiejar.CookieJar(); op = urllib.request.build_opener(urllib.request.HTTPCookieProcessor(cj))
    time.sleep(2)
    op.open(urllib.request.Request("http://127.0.0.1:8490/login", data=b"password=wifi-modem-test-1"))
    found = None
    for _ in range(12):
        time.sleep(2)
        t = json.loads(op.open("http://127.0.0.1:8490/api/telemetry").read())
        if t.get("modems"):
            found = t["modems"]
            break
    print("challenged:", state["challenged"], "| stats frames sent:", state["stats_sent"])
    print("server telemetry modems:", {k: dict(v["last"]) for k, v in (found or {}).items()})
    assert state["challenged"], "the server must ask the modem for its key"
    assert found and any(dict(v["last"]).get(40) == 5000 and dict(v["last"]).get(93) == 3 for v in found.values()), "both stats frames merged"
    print("RESULT: OK")
finally:
    p.kill()
    print("logs in", TMP)
