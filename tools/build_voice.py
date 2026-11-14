"""Bygger maarse-voice (Codec2) som WebAssembly og lægger den ind i
webklienten (maarse-client/index.html), komprimeret med rå deflate og
base64-kodet i <script id="voice-wasm">.

    python tools/build_voice.py
"""
import base64, os, re, subprocess, sys, zlib

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
env = dict(os.environ,
           CARGO_PROFILE_RELEASE_OPT_LEVEL="z", CARGO_PROFILE_RELEASE_LTO="true",
           CARGO_PROFILE_RELEASE_STRIP="true", CARGO_PROFILE_RELEASE_PANIC="abort",
           CARGO_PROFILE_RELEASE_CODEGEN_UNITS="1")
subprocess.run(["cargo", "build", "-p", "maarse-voice", "--release", "--target", "wasm32-unknown-unknown",
                "--target-dir", os.path.join(ROOT, "target", "voice")], cwd=ROOT, env=env, check=True)
wasm = open(os.path.join(ROOT, "target", "voice", "wasm32-unknown-unknown", "release", "maarse_voice.wasm"), "rb").read()
c = zlib.compressobj(9, zlib.DEFLATED, -15)
packed = c.compress(wasm) + c.flush()
b64 = base64.b64encode(packed).decode()
lines = "\n".join(b64[i:i + 120] for i in range(0, len(b64), 120))

p = os.path.join(ROOT, "maarse-client", "index.html")
html = open(p, encoding="utf-8").read()
new, n = re.subn(r'(<script type="application/octet-stream" id="voice-wasm">\n)(.*?)(</script>)',
                 lambda m: m.group(1) + lines + "\n" + m.group(3), html, flags=re.S)
if n != 1:
    sys.exit("fandt ikke <script id=\"voice-wasm\"> i index.html")
open(p, "w", encoding="utf-8", newline="\n").write(new)
print(f"maarse-voice: {len(wasm)} bytes wasm, {len(packed)} komprimeret, {len(b64)} base64 -> index.html")
