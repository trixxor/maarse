# Test of the Wi-Fi modem key (maarse_core::modemauth) against maarse-server:
# a fake modem connects to the backbone port, sends "MM" and answers the
# challenge right, wrong or not at all.
#   python tools/modem_key_test.py [target/release]
import hashlib, os, socket, struct, subprocess, sys, tempfile, time

BIN = sys.argv[1] if len(sys.argv) > 1 else "target/release"
EXE = ".exe" if os.name == "nt" else ""
SERVER = os.path.abspath(os.path.join(BIN, "maarse-server" + EXE))
TMP = tempfile.mkdtemp(prefix="maarse-modemkey-test-")
PASS = "mast modem passphrase"


def key(p):
    return hashlib.sha256(b"maarse/modem-key" + p.encode()).digest()


def tag(k, nonce):
    return hashlib.sha256(b"maarse/modem-auth" + k + nonce).digest()[:16]


def start(name, port, extra):
    d = os.path.join(TMP, name)
    return subprocess.Popen([SERVER, "--data", d, "--listen", f"127.0.0.1:{port}", "--public-addr", f"127.0.0.1:{port}", "--http", f"127.0.0.1:{port + 4000}"] + extra,
                            stdout=open(d + ".log", "w"), stderr=subprocess.STDOUT)


def modem(port, answer):
    """Connects as a modem; returns True if the host keeps the connection."""
    s = socket.create_connection(("127.0.0.1", port), timeout=15)
    s.sendall(b"MM")
    s.settimeout(5)
    try:
        n = struct.unpack("<H", s.recv(2))[0]
        body = b""
        while len(body) < n:
            body += s.recv(n - len(body))
    except (socket.timeout, struct.error):
        body = b""
    if body.startswith(b"\xffMK") and answer is not None:
        t = tag(key(answer), body[3:])
        s.sendall(struct.pack("<H", 20) + b"\x7f\xffMR" + t)
    # Kept: the host sends nothing and does not close for a while.
    s.settimeout(12 if answer is None else 4)
    try:
        closed = s.recv(1) == b""
    except socket.timeout:
        closed = False
    except OSError:
        closed = True
    s.close()
    return not closed, body.startswith(b"\xffMK")


procs = []
try:
    procs.append(start("nokey", 4351, []))
    procs.append(start("withkey", 4352, ["--modem-key", PASS]))
    time.sleep(2)
    kept, asked = modem(4351, PASS)
    print(f"server without a key: modem kept {kept} (expected False)")
    assert not kept
    kept, asked = modem(4352, "wrong passphrase!!")
    print(f"wrong key: asked {asked}, kept {kept} (expected True, False)")
    assert asked and not kept
    kept, asked = modem(4352, PASS)
    print(f"right key: asked {asked}, kept {kept} (expected True, True)")
    assert asked and kept
    kept, asked = modem(4352, None)
    print(f"no answer (old firmware): kept {kept} (expected False)")
    assert not kept
    log = open(os.path.join(TMP, "withkey.log"), encoding="utf-8", errors="replace").read()
    assert "wrong modem key" in log, log[-500:]
    print("RESULT: OK")
finally:
    for p in procs:
        p.kill()
    print("logs in", TMP)
