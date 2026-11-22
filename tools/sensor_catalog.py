"""Builds and checks the sensor catalogue.

    python tools/sensor_catalog.py build [--version N]   -> sensors/catalog.json
    python tools/sensor_catalog.py check                  -> exit 1 if sensors/catalog.json is out of date

The sources are one JSON file per sensor in sensors/catalog/ and the boards in
sensors/boards.json (see sensors/README.md). `build` merges them into the one
file that the host embeds and that CI signs and publishes. The deep checks (does
every description fit together, do the test vectors from the data sheet give
the right numbers) are done by the host's tests: `cargo test -p maarse-host sensors::catalog`.
"""
import json, os, sys, time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DIR = os.path.join(ROOT, "sensors")
OUT = os.path.join(DIR, "catalog.json")
SCHEMA = 1
BUSES = {"i2c", "analog", "digital_in", "digital_out"}


def load():
    boards = json.load(open(os.path.join(DIR, "boards.json"), encoding="utf-8"))
    items = []
    folder = os.path.join(DIR, "catalog")
    for name in sorted(os.listdir(folder)):
        if not name.endswith(".json"):
            continue
        item = json.load(open(os.path.join(folder, name), encoding="utf-8"))
        if item.get("id") != name[:-5]:
            sys.exit(f"{name}: the id must be the file name ({item.get('id')!r})")
        for key in ("id", "name", "group", "bus"):
            if not isinstance(item.get(key), str) or not item[key]:
                sys.exit(f"{name}: missing {key}")
        if item["bus"] not in BUSES:
            sys.exit(f"{name}: unknown bus {item['bus']!r}")
        items.append(item)
    ids = [i["id"] for i in items]
    if len(set(ids)) != len(ids):
        sys.exit("duplicate sensor ids")
    kinds = json.load(open(os.path.join(DIR, "kinds.json"), encoding="utf-8"))
    templates = []
    tfolder = os.path.join(DIR, "templates")
    for name in sorted(os.listdir(tfolder)):
        if name.endswith(".json"):
            t = json.load(open(os.path.join(tfolder, name), encoding="utf-8"))
            if t.get("id") != name[:-5] or not all(isinstance(t.get(k), str) and t[k] for k in ("title", "text")) or not isinstance(t.get("setup"), dict):
                sys.exit(f"templates/{name}: needs id (= file name), title, text and setup")
            templates.append(t)
    templates.sort(key=lambda t: (t.get("order", 50), t["id"]))
    return boards, items, kinds, templates


def bundle(version):
    boards, items, kinds, templates = load()
    return {"schema": SCHEMA, "version": version, "boards": boards, "kinds": kinds, "templates": templates, "sensors": items}


def text(doc):
    return json.dumps(doc, indent=1, ensure_ascii=False, sort_keys=True) + "\n"


def main():
    cmd = sys.argv[1] if len(sys.argv) > 1 else ""
    if cmd == "build":
        version = int(sys.argv[sys.argv.index("--version") + 1]) if "--version" in sys.argv else int(time.time())
        with open(OUT, "w", encoding="utf-8", newline="\n") as f:
            f.write(text(bundle(version)))
        print(f"sensors/catalog.json: version {version}")
    elif cmd == "check":
        have = json.load(open(OUT, encoding="utf-8"))
        want = bundle(have["version"])
        if have != want:
            sys.exit("sensors/catalog.json is out of date: run python tools/sensor_catalog.py build")
        print("sensors/catalog.json is up to date")
    else:
        sys.exit(__doc__)


main()
