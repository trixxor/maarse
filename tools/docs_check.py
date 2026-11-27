#!/usr/bin/env python3
"""Documentation checks for Maarse.

Usage: python tools/docs_check.py [--strict-all] [--no-settings]

Checks:
  1. Relative Markdown links and #anchors resolve. Pages in the new structure
     (index, architecture, concepts, guides, reference, security, decisions,
     tutorials, verification) must be clean (errors). Older pages are reported
     as warnings, or as errors with --strict-all.
  2. Pages in the new structure start with a title and have a "Type:" or
     "Status:" line near the top (index.md is exempt).
  3. docs/reference/settings.md is up to date (needs a built maarse-host;
     skipped with --no-settings or when no binary is found).

Exit code 1 if any error was found.
"""
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
NEW_DIRS = ["architecture", "concepts", "guides", "reference", "security",
            "decisions", "tutorials", "verification"]
NEW_FILES = {"docs/index.md"}
LINK = re.compile(r"(?<!!)\[[^\]]*\]\(([^)\s]+)(?:\s+\"[^\"]*\")?\)|!\[[^\]]*\]\(([^)\s]+)\)")
HEADING = re.compile(r"^(#{1,6})\s+(.*?)\s*#*\s*$")


def is_new(path):
    rel = path.relative_to(ROOT).as_posix()
    return rel in NEW_FILES or any(rel.startswith(f"docs/{d}/") for d in NEW_DIRS)


def slug(text):
    text = re.sub(r"`([^`]*)`", r"\1", text)
    text = re.sub(r"\[([^\]]*)\]\([^)]*\)", r"\1", text)
    text = re.sub(r"<[^>]+>", "", text)
    text = text.strip().lower()
    text = re.sub(r"[^\w\- ]", "", text, flags=re.UNICODE)
    return text.replace(" ", "-")


def anchors(path, cache={}):
    if path in cache:
        return cache[path]
    seen, out, fence = {}, set(), False
    for line in path.read_text(encoding="utf-8", errors="replace").splitlines():
        if line.lstrip().startswith("```"):
            fence = not fence
            continue
        if fence:
            continue
        m = HEADING.match(line)
        if m:
            s = slug(m.group(2))
            n = seen.get(s, 0)
            seen[s] = n + 1
            out.add(s if n == 0 else f"{s}-{n}")
    for m in re.finditer(r'<a\s+(?:name|id)="([^"]+)"', path.read_text(encoding="utf-8", errors="replace")):
        out.add(m.group(1))
    cache[path] = out
    return out


def check_links(path):
    problems = []
    fence = False
    for no, line in enumerate(path.read_text(encoding="utf-8", errors="replace").splitlines(), 1):
        if line.lstrip().startswith("```"):
            fence = not fence
            continue
        if fence:
            continue
        line = re.sub(r"`[^`]*`", "", line)
        for m in LINK.finditer(line):
            target = m.group(1) or m.group(2)
            if re.match(r"^(https?:|mailto:|data:|tel:)", target):
                continue
            target = target.split("?")[0]
            file_part, _, frag = target.partition("#")
            dest = path if not file_part else (path.parent / file_part).resolve()
            if file_part and not dest.exists():
                problems.append((no, f"missing file: {target}"))
                continue
            if frag and dest.suffix == ".md" and dest.is_file():
                if frag not in anchors(dest):
                    problems.append((no, f"missing anchor: {target}"))
    return problems


def check_header(path):
    rel = path.relative_to(ROOT).as_posix()
    if rel == "docs/index.md" or not is_new(path):
        return []
    lines = path.read_text(encoding="utf-8", errors="replace").splitlines()
    while lines and (lines[0].startswith("<!--") or not lines[0].strip()):
        lines = lines[1:]  # generated pages start with a comment
    problems = []
    if not lines or not lines[0].startswith("# "):
        problems.append((1, "page must start with a '# Title' line"))
    head = "\n".join(lines[:8])
    if "Type:" not in head and "Status:" not in head:
        problems.append((1, "no 'Type:' or 'Status:' line in the first 8 lines"))
    return problems


def main():
    strict_all = "--strict-all" in sys.argv
    errors = warnings = 0
    files = [p for p in list((ROOT / "docs").rglob("*.md")) + [ROOT / "README.md", ROOT / "compare.md", ROOT / "AGENTS.md"]
             if p.is_file() and "node_modules" not in p.parts and "vendor" not in p.parts]
    for p in sorted(files):
        rel = p.relative_to(ROOT).as_posix()
        for no, msg in check_header(p) + check_links(p):
            if is_new(p) or strict_all:
                errors += 1
                print(f"ERROR   {rel}:{no}: {msg}")
            else:
                warnings += 1
                print(f"warning {rel}:{no}: {msg}")
    if "--no-settings" not in sys.argv:
        r = subprocess.run([sys.executable, str(ROOT / "tools" / "gen_settings_reference.py"), "--check"],
                           capture_output=True, text=True)
        out = (r.stdout + r.stderr).strip()
        if r.returncode != 0 and "binary not found" in out:
            print("skipped settings reference check (no maarse-host binary)")
        elif r.returncode != 0:
            errors += 1
            print("ERROR   " + out)
        else:
            print(out)
    print(f"\n{len(files)} files checked: {errors} errors, {warnings} warnings (older pages)")
    sys.exit(1 if errors else 0)


if __name__ == "__main__":
    main()
