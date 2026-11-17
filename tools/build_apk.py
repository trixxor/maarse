"""Builds the native Må:rse app for Android (APK).

    python tools/build_apk.py            -> dist/maarse.apk

The app is a Kotlin/Compose project in android/ with maarse-host embedded as
a native library (crate maarse-android, built by cargo-ndk from Gradle).

Requires: the Android SDK (ANDROID_HOME, otherwise %LOCALAPPDATA%/Android/Sdk)
with platform 35 and NDK 27.2.12479018, a JDK 17+ (JAVA_HOME or java in PATH),
Rust with the aarch64-linux-android and x86_64-linux-android targets, and
cargo-ndk (cargo install cargo-ndk).

The APK is signed with a debug key (~/.android/debug.keystore, created if
missing), or with MAARSE_APK_KEYSTORE/MAARSE_APK_PASS for a release.
"""
import os, shutil, subprocess, sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
APP = os.path.join(ROOT, "android")
EXE = ".exe" if os.name == "nt" else ""
BAT = ".bat" if os.name == "nt" else ""


def find_sdk():
    for c in [os.environ.get("ANDROID_HOME"), os.environ.get("ANDROID_SDK_ROOT"),
              os.path.join(os.environ.get("LOCALAPPDATA", ""), "Android", "Sdk"),
              os.path.expanduser("~/Android/Sdk"), "C:/Android/Sdk"]:
        if c and os.path.isdir(os.path.join(c, "build-tools")):
            return c
    sys.exit("Android SDK not found (set ANDROID_HOME)")


def newest(path):
    def key(n):
        return [int(x) if x.isdigit() else 0 for x in n.split(".")]
    return os.path.join(path, sorted(os.listdir(path), key=key)[-1])


def tool(name):
    jh = os.environ.get("JAVA_HOME")
    if jh and os.path.exists(os.path.join(jh, "bin", name + EXE)):
        return os.path.join(jh, "bin", name + EXE)
    p = shutil.which(name)
    if not p:
        sys.exit(f"{name} not found (install a JDK and set JAVA_HOME)")
    return p


def run(*args, **kw):
    r = subprocess.run([str(a) for a in args], capture_output=True, text=True, **kw)
    if r.returncode != 0:
        sys.exit(f"ERROR: {' '.join(map(str, args[:3]))}\n{r.stdout[-4000:]}\n{r.stderr[-4000:]}")
    return r.stdout


sdk = find_sdk()
bt = newest(os.path.join(sdk, "build-tools"))
env = dict(os.environ, ANDROID_HOME=sdk, ANDROID_NDK_HOME=os.path.join(sdk, "ndk", "27.2.12479018"))
lp = os.path.join(APP, "local.properties")
if not os.path.exists(lp):
    with open(lp, "w") as f:
        f.write("sdk.dir=" + sdk.replace("\\", "/") + "\n")

print("Gradle: assembleRelease (with maarse-host via cargo-ndk)")
run(os.path.join(APP, "gradlew" + BAT), "assembleRelease", "--console=plain", cwd=APP, env=env)
unsigned = os.path.join(APP, "app", "build", "outputs", "apk", "release", "app-release-unsigned.apk")

# Release: MAARSE_APK_KEYSTORE + MAARSE_APK_PASS (e.g. from tools/release.py);
# otherwise the debug key.
ks = os.environ.get("MAARSE_APK_KEYSTORE") or os.path.expanduser("~/.android/debug.keystore")
ks_pass = os.environ.get("MAARSE_APK_PASS", "android")
if not os.environ.get("MAARSE_APK_KEYSTORE") and not os.path.exists(ks):
    os.makedirs(os.path.dirname(ks), exist_ok=True)
    run(tool("keytool"), "-genkeypair", "-keystore", ks, "-storepass", "android", "-keypass", "android",
        "-alias", "androiddebugkey", "-keyalg", "RSA", "-keysize", "2048", "-validity", "10000",
        "-dname", "CN=Android Debug,O=Android,C=US")
os.makedirs(os.path.join(ROOT, "dist"), exist_ok=True)
aligned = os.path.join(APP, "app", "build", "aligned.apk")
run(os.path.join(bt, "zipalign" + EXE), "-f", "-p", "4", unsigned, aligned)
apk = os.path.join(ROOT, "dist", "maarse.apk")
run(os.path.join(bt, "apksigner" + BAT), "sign", "--ks", ks, "--ks-pass", "pass:" + ks_pass,
    "--key-pass", "pass:" + ks_pass, "--out", apk, aligned)
run(os.path.join(bt, "apksigner" + BAT), "verify", apk)
print(f"Done: {apk} ({os.path.getsize(apk) // 1024} KB)")
