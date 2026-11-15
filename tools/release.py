"""Builds and signs a Må:rse release (Windows).

    python tools/release.py 0.5.0            -> dist/release-0.5.0/
    python tools/release.py --verify dist/release-0.5.0

The keys live in ~/.maarse/release/ (never in git) and are made the first time:
  release.key       Ed25519 release key (firmware updates and SHA256SUMS)
  release.pub       the public key (hex); built into the firmware
  android.keystore  key for the APK; android.pass is its password
Back up the folder. If you lose release.key, nodes in the field can no longer
be updated over the air with new releases.

The release contains firmware for all four boards (with the release key
built in and signed OTA packages), maarse-host.exe (with IP tunnel),
maarse.apk and SHA256SUMS + SHA256SUMS.sig.
"""
import os, secrets, shutil, subprocess, sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
KEYS = os.path.expanduser("~/.maarse/release")
BOARDS = ["xiao-nrf52840", "wismesh-tag", "heltec-v4", "xiao-esp32s3"]
SIGN = [os.path.join(ROOT, "target", "release", "maarse-sign.exe")]


def run(args, env=None, shell=False):
    print("  $", args if isinstance(args, str) else " ".join(args))
    r = subprocess.run(args, cwd=ROOT, env=env, shell=shell)
    if r.returncode != 0:
        sys.exit(f"FAILED ({r.returncode})")


def keys():
    os.makedirs(KEYS, exist_ok=True)
    key, pub = os.path.join(KEYS, "release.key"), os.path.join(KEYS, "release.pub")
    if not os.path.exists(key):
        print("Making release key in", KEYS)
        out = subprocess.run(SIGN + ["keygen", key], capture_output=True, text=True, check=True).stdout
        hexkey = [l.split("=", 1)[1].strip() for l in out.splitlines() if "MAARSE_RELEASE_PUBKEY=" in l][0]
        open(pub, "w").write(hexkey + "\n")
    ks, pw = os.path.join(KEYS, "android.keystore"), os.path.join(KEYS, "android.pass")
    if not os.path.exists(ks):
        print("Making Android key in", KEYS)
        password = secrets.token_urlsafe(24)
        open(pw, "w").write(password + "\n")
        keytool = os.path.join(os.environ.get("JAVA_HOME", ""), "bin", "keytool.exe")
        if not os.path.exists(keytool):
            keytool = "keytool"
        subprocess.run([keytool, "-genkeypair", "-keystore", ks, "-storepass", password, "-keypass", password,
                        "-alias", "maarse", "-keyalg", "RSA", "-keysize", "4096", "-validity", "10000",
                        "-dname", "CN=Maarse, O=Maarse"], check=True, capture_output=True)
    return key, open(pub).read().strip(), ks, open(pw).read().strip()


def verify(folder):
    pub = open(os.path.join(KEYS, "release.pub")).read().strip()
    run(SIGN + ["verify-release", "--pubkey", pub, folder])


def main():
    if len(sys.argv) == 3 and sys.argv[1] == "--verify":
        return verify(sys.argv[2])
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    version = sys.argv[1]
    run(["cargo", "build", "--release", "-p", "maarse-uf2", "--bin", "maarse-sign"])
    key, pub, ks, ks_pass = keys()
    print(f"Release {version}, public key {pub}")
    out = os.path.join(ROOT, "dist", f"release-{version}")
    shutil.rmtree(out, ignore_errors=True)
    os.makedirs(out)

    env = dict(os.environ, MAARSE_RELEASE_PUBKEY=pub, MAARSE_SIGN_KEY=key, MAARSE_FW_VERSION=version)
    for b in BOARDS:
        print(f"Firmware: {b}")
        pre = ". ~/export-esp.ps1; " if b in ("heltec-v4", "xiao-esp32s3") else ""
        run(["powershell", "-NoProfile", "-ExecutionPolicy", "Bypass", "-Command",
             f"{pre}& .\\scripts\\build-firmware.ps1 {b}"], env=env)
        for name in [f"maarse-{b}.uf2", f"maarse-{b}.bin", f"maarse-{b}-ota.bin", f"maarse-{b}-ota.manifest"]:
            src = os.path.join(ROOT, "dist", name)
            if os.path.exists(src):
                shutil.copy(src, os.path.join(out, name.replace(f"maarse-{b}", f"maarse-{version}-{b}")))

    print("maarse-host")
    run(["cargo", "build", "--release", "-p", "maarse-host", "--features", "tun"])
    shutil.copy(os.path.join(ROOT, "target", "release", "maarse-host.exe"), os.path.join(out, f"maarse-host-{version}.exe"))

    print("Android")
    run([sys.executable, os.path.join("tools", "build_apk.py")], env=dict(os.environ, MAARSE_APK_KEYSTORE=ks, MAARSE_APK_PASS=ks_pass))
    shutil.copy(os.path.join(ROOT, "dist", "maarse.apk"), os.path.join(out, f"maarse-{version}.apk"))

    run(SIGN + ["sign-release", "--key", key, out])
    verify(out)
    print(f"Done: {out}")


main()
