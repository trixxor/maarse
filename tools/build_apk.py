"""Bygger Må:rse-appen til Android (APK) uden Gradle, direkte med SDK'ens
værktøjer: aapt2, javac, d8, zipalign og apksigner.

    python tools/build_apk.py            -> dist/maarse.apk

Webklienten (maarse-client/) lægges ind som assets. Kræver Android SDK
(ANDROID_HOME, ellers %LOCALAPPDATA%/Android/Sdk) med build-tools og en
platform (35+), samt en JDK (JAVA_HOME eller javac i PATH). APK'en signeres
med en debug-nøgle (~/.android/debug.keystore, laves hvis den mangler).
"""
import glob, os, shutil, subprocess, sys, zipfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
APP = os.path.join(ROOT, "android")
OUT = os.path.join(ROOT, "target", "android")
EXE = ".exe" if os.name == "nt" else ""
BAT = ".bat" if os.name == "nt" else ""


def find_sdk():
    for c in [os.environ.get("ANDROID_HOME"), os.environ.get("ANDROID_SDK_ROOT"),
              os.path.join(os.environ.get("LOCALAPPDATA", ""), "Android", "Sdk"),
              os.path.expanduser("~/Android/Sdk"), "C:/Android/Sdk"]:
        if c and os.path.isdir(os.path.join(c, "build-tools")):
            return c
    sys.exit("Android SDK ikke fundet (sæt ANDROID_HOME)")


def newest(path, minimum=0):
    def key(n):
        return [int(x) if x.isdigit() else 0 for x in n.replace("android-", "").split(".")]
    items = [n for n in os.listdir(path) if key(n)[0] >= minimum]
    if not items:
        sys.exit(f"intet i {path}")
    return os.path.join(path, sorted(items, key=key)[-1])


def tool(name):
    jh = os.environ.get("JAVA_HOME")
    if jh and os.path.exists(os.path.join(jh, "bin", name + EXE)):
        return os.path.join(jh, "bin", name + EXE)
    p = shutil.which(name)
    if not p:
        sys.exit(f"{name} ikke fundet (installér en JDK og sæt JAVA_HOME)")
    return p


def run(*args):
    r = subprocess.run([str(a) for a in args], capture_output=True, text=True)
    if r.returncode != 0:
        sys.exit(f"FEJL: {' '.join(map(str, args[:2]))}\n{r.stdout}\n{r.stderr}")
    return r.stdout


sdk = find_sdk()
bt = newest(os.path.join(sdk, "build-tools"), 34)
platform = newest(os.path.join(sdk, "platforms"), 35)
android_jar = os.path.join(platform, "android.jar")
print(f"SDK {sdk}\n  build-tools {os.path.basename(bt)}, platform {os.path.basename(platform)}")

shutil.rmtree(OUT, ignore_errors=True)
for d in ["res/mipmap", "assets/web", "gen", "classes", "dex"]:
    os.makedirs(os.path.join(OUT, d), exist_ok=True)

# Ressourcer: ikon fra webklienten.
shutil.copy(os.path.join(ROOT, "maarse-client", "icon-192.png"), os.path.join(OUT, "res", "mipmap", "ic_launcher.png"))
# Assets: hele webklienten.
for f in os.listdir(os.path.join(ROOT, "maarse-client")):
    shutil.copy(os.path.join(ROOT, "maarse-client", f), os.path.join(OUT, "assets", "web", f))

aapt2 = os.path.join(bt, "aapt2" + EXE)
run(aapt2, "compile", "--dir", os.path.join(OUT, "res"), "-o", os.path.join(OUT, "res.zip"))
base = os.path.join(OUT, "base.apk")
run(aapt2, "link", "-o", base, "-I", android_jar, "--manifest", os.path.join(APP, "AndroidManifest.xml"),
    "-A", os.path.join(OUT, "assets"), "--java", os.path.join(OUT, "gen"), "--auto-add-overlay",
    os.path.join(OUT, "res.zip"))

sources = glob.glob(os.path.join(APP, "src", "**", "*.java"), recursive=True) + \
          glob.glob(os.path.join(OUT, "gen", "**", "*.java"), recursive=True)
run(tool("javac"), "-source", "11", "-target", "11", "-nowarn", "-Xlint:-options", "-encoding", "UTF-8",
    "-classpath", android_jar, "-d", os.path.join(OUT, "classes"), *sources)
classes = glob.glob(os.path.join(OUT, "classes", "**", "*.class"), recursive=True)
run(os.path.join(bt, "d8" + BAT), "--release", "--min-api", "26", "--lib", android_jar,
    "--output", os.path.join(OUT, "dex"), *classes)

unsigned = os.path.join(OUT, "unsigned.apk")
shutil.copy(base, unsigned)
with zipfile.ZipFile(unsigned, "a", zipfile.ZIP_DEFLATED) as z:
    z.write(os.path.join(OUT, "dex", "classes.dex"), "classes.dex")
aligned = os.path.join(OUT, "aligned.apk")
run(os.path.join(bt, "zipalign" + EXE), "-f", "-p", "4", unsigned, aligned)

ks = os.path.expanduser("~/.android/debug.keystore")
if not os.path.exists(ks):
    os.makedirs(os.path.dirname(ks), exist_ok=True)
    run(tool("keytool"), "-genkeypair", "-keystore", ks, "-storepass", "android", "-keypass", "android",
        "-alias", "androiddebugkey", "-keyalg", "RSA", "-keysize", "2048", "-validity", "10000",
        "-dname", "CN=Android Debug,O=Android,C=US")
os.makedirs(os.path.join(ROOT, "dist"), exist_ok=True)
apk = os.path.join(ROOT, "dist", "maarse.apk")
run(os.path.join(bt, "apksigner" + BAT), "sign", "--ks", ks, "--ks-pass", "pass:android",
    "--key-pass", "pass:android", "--out", apk, aligned)
run(os.path.join(bt, "apksigner" + BAT), "verify", apk)
print(f"Færdig: {apk} ({os.path.getsize(apk) // 1024} KB)")
