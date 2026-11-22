#!/usr/bin/env python3
"""Checks and helps with the language files in i18n/.

    python tools/i18n_check.py              check every language file (exit 1 on a problem)
    python tools/i18n_check.py --missing    also list texts in the web client, the server pages and the
                                            Android app that have no translation in a language
    python tools/i18n_check.py --new xx     start a new language: writes i18n/xx.json with every known
                                            English text and an empty translation (empty = stays English)

The English text in the code is the key, so a text is never renamed to be translated; see
docs/i18n.md. The "missing" list is a help: it looks for texts that people read, with a few
rules, and it can list a few code fragments as well.
"""
import json
import re
import sys
from html.parser import HTMLParser
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
I18N = ROOT / "i18n"
BS = chr(92)
TOKEN = re.compile(r"\{\d+\}|%(?:\.\d+)?[dsf]|%\d*d")


def norm(s):
    return re.sub(r"\s+", " ", s).strip()


def load(path):
    return json.loads(path.read_text(encoding="utf-8"))


def check_files():
    problems = []
    files = sorted(I18N.glob("*.json"))
    if not files:
        problems.append("no language files in i18n/")
    for path in files:
        try:
            data = load(path)
        except ValueError as e:
            problems.append(f"{path.name}: not valid JSON ({e})")
            continue
        meta = data.get("meta", {})
        if meta.get("code") != path.stem:
            problems.append(f"{path.name}: meta.code must be '{path.stem}'")
        if not meta.get("name"):
            problems.append(f"{path.name}: meta.name (the language's own name) is missing")
        seen = {}
        for key, value in data.get("texts", {}).items():
            if not isinstance(value, str):
                problems.append(f"{path.name}: the translation of {key[:50]!r} is not text")
                continue
            if norm(key) != key:
                problems.append(f"{path.name}: the key {key[:50]!r} has extra white space")
            if norm(key) in seen:
                problems.append(f"{path.name}: {key[:50]!r} appears twice")
            seen[norm(key)] = True
            if not value:
                continue
            want = sorted(TOKEN.findall(key))
            have = sorted(TOKEN.findall(value))
            extra = list(have)
            for t in want:
                if t in extra:
                    extra.remove(t)
            if extra:
                problems.append(f"{path.name}: {key[:50]!r} has {extra} in the translation that the English text does not have")
    return problems, files


# ---------- what the code says (for --missing and --new) ----------

class Html(HTMLParser):
    def __init__(self):
        super().__init__()
        self.out = []
        self.skip = 0

    def handle_starttag(self, tag, attrs):
        if tag in ("script", "style"):
            self.skip += 1
        for k, v in attrs:
            if k in ("title", "placeholder", "aria-label", "alt") and v:
                self.out.append(v)

    def handle_endtag(self, tag):
        if tag in ("script", "style"):
            self.skip -= 1

    def handle_data(self, data):
        if not self.skip and data.strip():
            self.out.append(data.strip())


JS_STRING = re.compile(
    '"((?:[^"' + BS + BS + "\\n]|" + BS + BS + '.)*)"|' +
    "'((?:[^'" + BS + BS + "\\n]|" + BS + BS + ".)*)'|" +
    "`((?:[^`" + BS + BS + "]|" + BS + BS + ".)*)`"
)
BAD = re.compile(r"function |const |let |=>|\);|\) \{|return |\bvar\(|rgba|^[,;:)=>?\]]|[{;]\s*$|^//|\bnew [A-Z]|\.push\(|\.length|\bif \(|===|\|\||&&|\bel\(|\.style|document\.|^\W*$")


def unescape(s):
    s = s.replace(BS + "n", "\n").replace(BS + "t", "\t").replace(BS + '"', '"').replace(BS + "'", "'")
    return s.replace(BS + "$", "$").replace(BS + BS, BS)


def human(t):
    """A text that a person reads, as opposed to code, a CSS value, a command or an identifier."""
    if not re.search(r"[A-Za-z]{2}", t) or BAD.search(t) or len(t) > 400:
        return False
    if re.match(r"^[a-z][a-z0-9_.:/#\-]*$", t) or re.match(r"^[A-Z0-9_]+$", t) or re.match(r"^[a-z]+[A-Z]\w*$", t):
        return False
    if re.search(r"[;{}]|=\s*\"", t) and not re.search(r"\s[a-z]{2,}\s", t):
        return False
    return bool(re.search(r"[A-Z]|\s", t))


def template_to_key(t):
    n = [0]

    def sub(_):
        n[0] += 1
        return "{%d}" % (n[0] - 1)
    return norm(re.sub(r"\$\{[^}]*\}", sub, t))


def js_texts(source):
    out = set()
    for m in JS_STRING.finditer(source):
        s = next(g for g in m.groups() if g is not None)
        pieces = [p for p in re.split(r"<[^>]*>", s)] if "<" in s and ">" in s else [s]
        for p in pieces:
            p = template_to_key(unescape(p))
            if human(p):
                out.add(p)
    return out


def web_texts():
    out = set()
    for name in ("index.html", "server.html", "flash.html"):
        text = (ROOT / "maarse-client" / name).read_text(encoding="utf-8")
        parser = Html()
        parser.feed(text)
        out |= {template_to_key(x) for x in parser.out if human(template_to_key(x))}
        for m in re.finditer(r"<script>(.*?)</script>", text, re.S):
            if "BEGIN-VOICE-WASM" not in m.group(1)[:200] and len(m.group(1)) < 3_000_000:
                out |= js_texts(m.group(1))
    admin = (ROOT / "maarse-host" / "src" / "admin.rs").read_text(encoding="utf-8")
    if 'LOGIN_HTML' in admin:
        part = admin[admin.index("LOGIN_HTML"):]
        part = part[part.index('r#"') + 3: part.index('"#')]
        parser = Html()
        parser.feed(part)
        out |= {template_to_key(x) for x in parser.out if human(template_to_key(x))}
    return out


def kotlin_texts():
    out = set()
    base = ROOT / "android" / "app" / "src" / "main" / "java" / "dk" / "maarse" / "app"
    for path in sorted(list(base.glob("*.kt")) + list((base / "ui").glob("*.kt"))):
        src = path.read_text(encoding="utf-8")
        for tpl in kotlin_strings(src):
            t = template_to_key(unescape(tpl))
            if human(t):
                out.add(t)
    return out


def kotlin_strings(src):
    """Every normal string literal in Kotlin source, with ${..} and $name kept as ${x}."""
    i, n = 0, len(src)
    found = []
    while i < n:
        if src.startswith("//", i):
            while i < n and src[i] != "\n":
                i += 1
        elif src.startswith("/*", i):
            j = src.find("*/", i + 2)
            i = n if j < 0 else j + 2
        elif src.startswith('"""', i):
            j = src.find('"""', i + 3)
            i = n if j < 0 else j + 3
        elif src[i] == "'":
            j = i + (3 if src[i + 1:i + 2] == BS else 2)
            while j < n and src[j] != "'" and src[j] != "\n":
                j += 1
            i = j + 1
        elif src[i] == '"':
            s, i = read_kotlin_string(src, i + 1)
            found.append(s)
        else:
            i += 1
    return found


def read_kotlin_string(src, i):
    n, buf = len(src), []
    while i < n:
        c = src[i]
        if c == BS:
            buf.append(src[i:i + 2])
            i += 2
        elif c == '"':
            return "".join(buf), i + 1
        elif c == "$" and src[i + 1:i + 2] == "{":
            depth, j = 1, i + 2
            while j < n and depth:
                if src[j] == "{":
                    depth += 1
                elif src[j] == "}":
                    depth -= 1
                elif src[j] == '"':
                    _, j = read_kotlin_string(src, j + 1)
                    continue
                j += 1
            buf.append("${x}")
            i = j
        elif c == "$" and re.match(r"[A-Za-z_]", src[i + 1:i + 2] or " "):
            m = re.match(r"\$[A-Za-z_]\w*", src[i:])
            buf.append("${x}")
            i += len(m.group(0))
        else:
            buf.append(c)
            i += 1
    return "".join(buf), i


def sensor_texts():
    """Texts in the sensor catalogue and templates (sensors/*.json): titles, notes, alarm texts."""
    keys = {"title", "text", "note", "group", "name", "grove", "label", "hint", "description", "help", "summary"}
    out = set()

    def walk(o, k=None):
        if isinstance(o, dict):
            for a, b in o.items():
                walk(b, a)
        elif isinstance(o, list):
            for x in o:
                walk(x, k)
        elif isinstance(o, str) and k in keys and re.search(r"[A-Za-z]{3}", o):
            out.add(norm(o))
    for f in sorted((ROOT / "sensors").rglob("*.json")):
        walk(load(f))
    return out


def main(argv):
    problems, files = check_files()
    for p in problems:
        print("PROBLEM:", p)
    if "--new" in argv:
        code = argv[argv.index("--new") + 1]
        if not re.fullmatch(r"[a-z]{2,3}(-[a-z0-9]{2,8})?", code):
            print("The language code must look like 'de' or 'pt-br'.")
            return 2
        target = I18N / f"{code}.json"
        if target.exists():
            print(f"{target} exists already.")
            return 2
        keys = sorted({k for f in files for k in load(f).get("texts", {})})
        data = {"meta": {"code": code, "name": "TRANSLATE: the language's own name"}, "texts": {k: "" for k in keys}}
        target.write_text(json.dumps(data, ensure_ascii=False, indent=1) + "\n", encoding="utf-8")
        print(f"Wrote {target}: {len(keys)} texts to translate. An empty translation stays English.")
        return 0
    if "--missing" in argv:
        texts = sorted(web_texts() | kotlin_texts() | sensor_texts())
        for path in files:
            have = {norm(k) for k, v in load(path).get("texts", {}).items() if v}
            miss = [t for t in texts if t not in have]
            print(f"\n{path.name}: {len(miss)} of {len(texts)} texts in the code have no translation")
            for t in miss:
                print("   ", t[:140])
    print(f"\n{len(files)} language file(s) checked, {len(problems)} problem(s).")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
