#!/usr/bin/env python3
"""Validator for the Omnibus spec system.

Checks (see META.md "Validation"):
  1. required structure exists (META/INDEX/WORKFLOW, decisions/_template.md, populated dirs);
  2. INDEX.md lists every file under specs/;
  3. every relative markdown link inside specs/*.md resolves to an existing file;
  4. every heading of the three source design docs is mapped in tools/coverage.toml
     to at least one existing spec file (zero unmapped sections);
  5. spec content is English-only (no Cyrillic in specs/**/*.md);
  6. backticked repo-root paths inside specs/*.md either exist on disk (literal or glob)
     or sit on a line explicitly marked planned - META.md: unmarked paths must exist.

Exit 0 = green. Any finding prints and exits 1.
"""

import re
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SPEC = ROOT / "specs"
SRC = ROOT / "docs" / "development"

SOURCE_DOCS = ["omnibus-spec.md", "omnibus-architecture.md", "omnibus-roadmap.md"]
REQUIRED_FILES = ["META.md", "INDEX.md", "WORKFLOW.md", "decisions/_template.md"]
REQUIRED_DIRS = ["architecture", "domains", "contracts", "decisions", "tools"]

HEADING = re.compile(r"^(#{1,6})\s+(.+?)\s*$")
LINK = re.compile(r"\[[^\]]*\]\(([^)\s]+)\)")
FENCE = re.compile(r"^\s*(```|~~~)")
CYRILLIC = re.compile(r"[\u0400-\u04FF]")

errors: list[str] = []


def err(msg: str) -> None:
    errors.append(msg)


def check_structure() -> None:
    for rel in REQUIRED_FILES:
        if not (SPEC / rel).is_file():
            err(f"missing required file: specs/{rel}")
    for d in REQUIRED_DIRS:
        p = SPEC / d
        if not p.is_dir() or not any(p.iterdir()):
            err(f"missing or empty directory: specs/{d}/")


def source_headings(doc: str) -> list[str]:
    out = []
    for line in (SRC / doc).read_text(encoding="utf-8").splitlines():
        m = HEADING.match(line)
        if m:
            out.append(m.group(2).strip())
    return out


def check_coverage() -> int:
    cov = tomllib.loads((SPEC / "tools" / "coverage.toml").read_text(encoding="utf-8"))
    table = cov.get("coverage", {})
    mapped = 0
    for doc in SOURCE_DOCS:
        for head in source_headings(doc):
            key = f"{doc}#{head}"
            row = table.get(key)
            if row is None:
                err(f"unmapped source heading: {key}")
                continue
            if not isinstance(row, list) or not row:
                err(f"empty coverage mapping: {key}")
                continue
            for target in row:
                if not (SPEC / target).is_file():
                    err(f"coverage target does not exist: {key} -> {target}")
            mapped += 1
    return mapped


def check_index() -> int:
    index = (SPEC / "INDEX.md").read_text(encoding="utf-8")
    listed = 0
    for f in sorted(SPEC.rglob("*")):
        if not f.is_file():
            continue
        if "__pycache__" in f.parts:  # bytecode cache is not a spec file; INDEX lists specs only
            continue
        rel = f.relative_to(SPEC).as_posix()
        if rel not in index:
            err(f"INDEX.md does not list: {rel}")
        else:
            listed += 1
    return listed


def check_links() -> int:
    resolved = 0
    for f in sorted(SPEC.rglob("*.md")):
        in_fence = False
        for lineno, line in enumerate(f.read_text(encoding="utf-8").splitlines(), 1):
            if FENCE.match(line):
                in_fence = not in_fence
                continue
            if in_fence:
                continue
            for m in LINK.finditer(line):
                target = m.group(1)
                if target.startswith(("http://", "https://", "mailto:")):
                    continue
                path = target.split("#", 1)[0]
                if not path:
                    continue  # pure in-page anchor
                if (f.parent / path).is_file():
                    resolved += 1
                else:
                    where = f.relative_to(SPEC).as_posix()
                    err(f"broken link: {where}:{lineno} -> {target}")
    return resolved


def check_english() -> None:
    for f in sorted(SPEC.rglob("*.md")):
        for lineno, line in enumerate(f.read_text(encoding="utf-8").splitlines(), 1):
            if CYRILLIC.search(line):
                where = f.relative_to(SPEC).as_posix()
                err(f"cyrillic character in specs/{where}:{lineno}")


SPAN = re.compile(r"`([^`\n]+)`")
PLANNED = re.compile(r"\bplanned\b", re.IGNORECASE)
# a span is a repo-root path only when its first segment names a repo top-level entry
# (existing, or the planned `packaging/` dir from roadmap 1.8.1) or a planned crate;
# protocol methods (`tools/list`), operation enums (`start/stop/restart`) and env-rooted
# runtime paths stay out
TOPLEVEL = {p.name for p in ROOT.iterdir()} | {"packaging"}


def is_repo_path(span: str) -> bool:
    if "/" not in span or span.startswith("/"):
        return False
    first = span.split("/", 1)[0]
    return first in TOPLEVEL or first.startswith("omnibus-")


def path_exists(rel: str) -> bool:
    if "*" in rel or "?" in rel:
        return any(ROOT.glob(rel))
    return (ROOT / rel).exists()


def check_path_refs() -> int:
    checked = 0
    for f in sorted(SPEC.rglob("*.md")):
        in_fence = False
        for lineno, line in enumerate(f.read_text(encoding="utf-8").splitlines(), 1):
            if FENCE.match(line):
                in_fence = not in_fence
                continue
            if in_fence:
                continue  # fenced planned-layout blocks are exempt (META.md convention)
            for m in SPAN.finditer(line):
                span = m.group(1)
                if not is_repo_path(span):
                    continue
                checked += 1
                if path_exists(span):
                    continue
                if not PLANNED.search(line):
                    where = f.relative_to(SPEC).as_posix()
                    err(f"unmarked planned path: {where}:{lineno} -> `{span}`")
    return checked


def main() -> int:
    check_structure()
    mapped = check_coverage()
    listed = check_index()
    links = check_links()
    check_english()
    paths = check_path_refs()
    if errors:
        print(f"FAIL: {len(errors)} finding(s)")
        for e in errors:
            print(f" - {e}")
        return 1
    print(
        f"OK: structure green; {mapped} source headings mapped (zero unmapped); "
        f"INDEX lists {listed} files; {links} links resolve; English-only clean; "
        f"{paths} repo-path references verified (existing or marked planned)"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
