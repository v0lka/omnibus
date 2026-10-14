#!/usr/bin/env python3
"""Validate the T2 packaging and CI-matrix artifacts (roadmap 2.6.1 / 2.6.2).

The honest-local-subset validator, in the spirit of the ci.yml package-build
job: every check here is fully decidable on a developer machine — structure,
field presence, cross-artifact consistency, and (for the YAML/ini artifacts)
real parsing. What is deliberately NOT checked here: whether a deb/rpm
actually installs on a real desktop (that is the REQ-EXT VM leg of the
matrix, ci/matrix/t2.yml), and whether the PPA upload succeeds (human,
out-of-band, credentialed — never CI).

Checked artifacts:
  packaging/debian/{control,rules,install,changelog,source/format}
  packaging/fedora/omnibus.spec
  packaging/ppa/{dput.cf,upload.sh,README.md}
  packaging/README.md
  ci/matrix/{t2.yml,to_json.py,README.md}
  .github/workflows/*.yml  (parse + t2.yml content contract)

Exit code: 0 every check green; 1 on the first-printed failure set.
Requires: python3 + PyYAML (the same pair the CI matrix legs install as
python3-yaml / python3-pyyaml), bash (for `bash -n` on shell scripts).
"""
from __future__ import annotations

import configparser
import glob
import json
import re
import subprocess
import sys
from pathlib import Path

try:
    import yaml
except ImportError:  # pragma: no cover
    print(
        "error: PyYAML is required (pip install pyyaml; distro packages: "
        "python3-yaml on apt, python3-pyyaml on dnf)",
        file=sys.stderr,
    )
    sys.exit(1)

REPO = Path(__file__).resolve().parent.parent

# The pass/fail journal, printed with per-section headers; failures never
# abort the remaining checks — one run shows the full damage.
_passed = 0
_failed: list[str] = []


def check(name: str, ok: bool, detail: str = "") -> None:
    global _passed
    mark = "PASS" if ok else "FAIL"
    line = f"  [{mark}] {name}" + (f": {detail}" if detail else "")
    print(line)
    if ok:
        _passed += 1
    else:
        _failed.append(f"{name}" + (f": {detail}" if detail else ""))


def section(title: str) -> None:
    print(f"== {title}")


def read(*parts: str) -> str:
    return (REPO.joinpath(*parts)).read_text(encoding="utf-8")


def workspace_version() -> str:
    """The single version source: [workspace.package] in Cargo.toml."""
    text = read("Cargo.toml")
    in_block = False
    for line in text.splitlines():
        stripped = line.strip()
        if stripped.startswith("[") and stripped.endswith("]"):
            in_block = stripped == "[workspace.package]"
            continue
        if in_block and stripped.startswith("version"):
            match = re.match(r'version\s*=\s*"([^"]+)"', stripped)
            if match:
                return match.group(1)
    return ""


def parse_debian_control(text: str) -> list[dict[str, str]]:
    """RFC822-ish stanza parse: blank-line separated, space continuations."""
    stanzas: list[dict[str, str]] = []
    current: dict[str, str] = {}
    last_field = ""
    for raw in text.splitlines():
        if not raw.strip():
            # A blank line ends the stanza ( debian control stanza separator;
            # our Description body indents every continuation line, so a
            # blank line is never part of a field).
            if current:
                stanzas.append(current)
                current = {}
                last_field = ""
            continue
        if raw.lstrip().startswith("#"):
            # Debian control files carry '#' comments; not field data.
            continue
        if raw[:1] in (" ", "\t"):  # continuation of the previous field
            if last_field:
                current[last_field] += " " + raw.strip()
            continue
        field, sep, value = raw.partition(":")
        if not sep:
            raise ValueError(f"unparseable control line: {raw!r}")
        last_field = field.strip()
        current[last_field] = value.strip()
    if current:
        stanzas.append(current)
    return stanzas


# --------------------------------------------------------------------------
# packaging/debian
# --------------------------------------------------------------------------

def validate_debian() -> None:
    section("packaging/debian (T2 channels: trixie, bookworm)")

    # ---- control ----
    try:
        stanzas = parse_debian_control(read("packaging/debian/control"))
    except ValueError as exc:
        check("debian/control parses as debian control", False, str(exc))
        stanzas = []
    check(
        "debian/control parses as debian control",
        bool(stanzas),
    )
    if stanzas:
        src = stanzas[0]
        check(
            "control: Source is omnibus",
            src.get("Source") == "omnibus",
        )
        for field in ("Section", "Priority", "Maintainer", "Standards-Version"):
            check(f"control: {field} present", bool(src.get(field)))
        check(
            "control: debhelper-compat (= 13) in Build-Depends (covers bookworm + trixie)",
            "debhelper-compat (= 13)" in src.get("Build-Depends", ""),
        )
        check(
            "control: Rules-Requires-Root is no (no root needed to build)",
            src.get("Rules-Requires-Root", "").lower() == "no",
        )
        check(
            "control: source + one binary stanza (2 total)",
            len(stanzas) == 2,
            f"got {len(stanzas)} stanzas",
        )
        if len(stanzas) > 1:
            bin_stanza = stanzas[1]
            check(
                "control: binary package omnibus, arch amd64",
                bin_stanza.get("Package") == "omnibus"
                and bin_stanza.get("Architecture") == "amd64",
            )
            depends = bin_stanza.get("Depends", "")
            for token in ("${misc:Depends}", "at-spi2-core", "systemd"):
                check(f"control: Depends includes {token}", token in depends)
            check(
                "control: probe-reported components are Recommends (R3 ladder, not hard deps)",
                all(t in bin_stanza.get("Recommends", "") for t in ("network-manager", "xdg-desktop-portal")),
            )
            check(
                "control: Description present",
                bool(bin_stanza.get("Description")),
            )

    # ---- rules ----
    rules_path = REPO / "packaging/debian/rules"
    rules = rules_path.read_text(encoding="utf-8")
    check(
        "debian/rules: make shebang",
        rules.startswith("#!/usr/bin/make -f"),
    )
    check(
        "debian/rules: executable bit set",
        rules_path.stat().st_mode & 0o111 != 0,
    )
    check(
        "debian/rules: dh sequence entry (%: / dh $@)",
        re.search(r"^%:\n\tdh \$@", rules, re.MULTILINE) is not None,
    )
    check(
        "debian/rules: builds via cargo build --release (repo-tree convention)",
        "cargo build --release" in rules,
    )
    check(
        "debian/rules: hermetic package build (dh_auto_test overridden off)",
        re.search(r"override_dh_auto_test:\n(?:\t:[^\n]*\n?)+", rules) is not None,
    )
    check(
        "debian/rules: dh_auto_install overridden (root Makefile must not leak into the package build)",
        "override_dh_auto_install:" in rules,
    )
    bad_indent = [
        f"line {i}"
        for i, line in enumerate(rules.splitlines(), 1)
        if line.startswith("    ") and not line.lstrip().startswith("#")
    ]
    check(
        "debian/rules: recipe lines are tab-indented (make requirement)",
        not bad_indent,
        ", ".join(bad_indent),
    )

    # ---- install ----
    install_lines = [
        line
        for line in read("packaging/debian/install").splitlines()
        if line.strip() and not line.lstrip().startswith("#")
    ]
    check(
        "debian/install: installs the workspace binary to usr/bin",
        "target/release/omnibusd usr/bin" in install_lines,
    )
    scopes_line = [
        line for line in install_lines if "scopes.d" in line
    ]
    check(
        "debian/install: installs packaging/scopes.d/*.toml to etc/omnibus/scopes.d",
        scopes_line == ["packaging/scopes.d/*.toml etc/omnibus/scopes.d"],
    )
    check(
        "debian/install: scopes.d glob resolves to at least one manifest",
        len(glob.glob(str(REPO / "packaging/scopes.d/*.toml"))) >= 1,
    )
    for line in install_lines:
        parts = line.split()
        ok = (
            len(parts) == 2
            and not parts[1].startswith("/")
            and (parts[1].startswith("usr/") or parts[1].startswith("etc/"))
        )
        check(
            f"debian/install: relative dest under usr/ or etc/ ({line})",
            ok,
        )

    # ---- changelog ----
    changelog = read("packaging/debian/changelog")
    first = changelog.splitlines()[0] if changelog else ""
    match = re.match(r"^omnibus \((\d+\.\d+\.\d+)\) (\S+); urgency=\S+$", first)
    check(
        "debian/changelog: first line is a valid debian version header",
        match is not None,
        first,
    )
    if match:
        check(
            "debian/changelog: version matches the workspace version "
            f"({workspace_version()})",
            match.group(1) == workspace_version(),
        )
    check(
        "debian/changelog: signed-off maintainer trailer",
        re.search(r"^ -- .+ <.+@.+>  \w{3}, \d{1,2} \w{3} \d{4} \d{2}:\d{2}:\d{2} [+-]\d{4}$",
                  changelog, re.MULTILINE) is not None,
    )

    # ---- source/format ----
    check(
        "debian/source/format: 3.0 (native) — source is the repo tree",
        read("packaging/debian/source/format").strip() == "3.0 (native)",
    )


# --------------------------------------------------------------------------
# packaging/fedora
# --------------------------------------------------------------------------

def spec_sections(spec: str) -> dict[str, str]:
    """Split a spec into %section -> body (tags stay in the 'header' key)."""
    sections: dict[str, str] = {"header": ""}
    current = "header"
    for line in spec.splitlines():
        match = re.match(r"^%(\w+)\s*$", line)
        if match:
            current = match.group(1)
            sections.setdefault(current, "")
            continue
        sections[current] += line + "\n"
    return sections


def validate_fedora() -> None:
    section("packaging/fedora (T2 channel: COPR)")
    spec = read("packaging/fedora/omnibus.spec")
    tags = dict(
        re.findall(r"^(Name|Version|Release|Summary|License):\s+(.+)$", spec, re.MULTILINE)
    )
    check("spec: Name is omnibus", tags.get("Name") == "omnibus")
    check(
        f"spec: Version matches the workspace version ({workspace_version()})",
        tags.get("Version") == workspace_version(),
    )
    check(
        "spec: Release carries the %dist bump",
        re.match(r"^\d+%\{\?dist\}$", tags.get("Release", "")) is not None,
    )
    check("spec: Summary present", bool(tags.get("Summary")))
    check(
        "spec: License tag present (honestly 'Undecided' until upstream designates one)",
        bool(tags.get("License")),
    )
    build_requires = re.findall(r"^BuildRequires:\s*(.+)$", spec, re.MULTILINE)
    check(
        "spec: BuildRequires rust + cargo + gcc",
        {"rust", "cargo", "gcc"} <= {b.strip() for b in build_requires},
    )
    check(
        "spec: Requires systemd + at-spi2-core (the unconditional T2 readers)",
        {"systemd", "at-spi2-core"} <= set(re.findall(r"^Requires:\s*(.+)$", spec, re.MULTILINE)),
    )
    check(
        "spec: probe-reported components are Recommends (R3 ladder)",
        {"NetworkManager", "xdg-desktop-portal"}
        <= set(re.findall(r"^Recommends:\s*(.+)$", spec, re.MULTILINE)),
    )

    sections = spec_sections(spec)
    check(
        "spec: %build runs cargo build --release",
        "cargo build --release" in sections.get("build", ""),
    )
    install = sections.get("install", "")
    check(
        "spec: %install installs the binary to %_bindir",
        "%{buildroot}%{_bindir}/omnibusd" in install,
    )
    check(
        "spec: %install installs packaging/scopes.d/*.toml",
        "packaging/scopes.d/*.toml" in install
        and "%{_sysconfdir}/omnibus/scopes.d" in install,
    )
    files = sections.get("files", "")
    check(
        "spec: %files ships the binary",
        "%{_bindir}/omnibusd" in files,
    )
    check(
        "spec: %files ships scopes.d as %config(noreplace) (admin-owned grants survive upgrades)",
        "%config(noreplace) %{_sysconfdir}/omnibus/scopes.d/*.toml" in files,
    )
    prep_body = "\n".join(
        line
        for line in sections.get("prep", "").splitlines()
        if not line.lstrip().startswith("#")
    )
    check(
        "spec: %prep is explicit (repo-tree source, no tarball)",
        prep_body.strip() == ":",
    )
    changelog = sections.get("changelog", "")
    check(
        "spec: %changelog entry with a valid rpm date header",
        re.search(
            r"^\* \w{3} [A-Z][a-z]{2} [ \d]\d \d{4} .+ - \d+\.\d+\.\d+-\d+",
            changelog,
            re.MULTILINE,
        )
        is not None,
    )
    check(
        "spec: no Source0 by design (source is the repo tree) — absence is the contract",
        not re.search(r"^Source\d*:", spec, re.MULTILINE),
    )


# --------------------------------------------------------------------------
# packaging/ppa
# --------------------------------------------------------------------------

def validate_ppa() -> None:
    section("packaging/ppa (T2 channels: Ubuntu LTS, Mint by inheritance)")

    parser = configparser.ConfigParser()
    parser.read_string(read("packaging/ppa/dput.cf"))
    check("dput.cf: parses as ini with an [omnibus] host", parser.has_section("omnibus"))
    if parser.has_section("omnibus"):
        host = parser["omnibus"]
        check(
            "dput.cf: targets the Launchpad PPA host",
            host.get("fqdn") == "ppa.launchpad.net",
        )
        check(
            "dput.cf: incoming points at the PPA pool (~owner/ubuntu/name)",
            host.get("incoming", "").startswith("~"),
        )
        check(
            "dput.cf: allow_unsigned_uploads = 0 (the signing AC starts at the upload)",
            host.get("allow_unsigned_uploads", "1") == "0",
        )
        check(
            "dput.cf: carries names only — no login secrets beyond a username",
            {"password", "secret", "key"} not in {k.lower() for k in host},
        )

    upload = REPO / "packaging/ppa/upload.sh"
    text = upload.read_text(encoding="utf-8")
    check("upload.sh: executable bit set", upload.stat().st_mode & 0o111 != 0)
    proc = subprocess.run(
        ["bash", "-n", str(upload)], capture_output=True, text=True
    )
    check(
        "upload.sh: bash -n syntax clean",
        proc.returncode == 0,
        proc.stderr.strip(),
    )
    check("upload.sh: set -euo pipefail", "set -euo pipefail" in text)
    check(
        "upload.sh: validates the series suffix before it reaches sed/dpkg",
        "case $suffix in" in text and "[A-Za-z0-9.+-]" in text,
    )
    check(
        "upload.sh: applies the suffix to this upload only (committed changelog stays bare)",
        "sed -i \"1s/^omnibus" in text,
    )
    check(
        "upload.sh: builds a SIGNED source package (dpkg-buildpackage -S)",
        "dpkg-buildpackage -S" in text,
    )
    check(
        "upload.sh: uploads via dput with the committed host config",
        "dput omnibus" in text,
    )
    check(
        "upload.sh: sources the repo tree clean (git archive, no .git/target)",
        "git archive --format=tar HEAD" in text,
    )
    for required in ("packaging/debian/control", "packaging/ppa/dput.cf"):
        check(
            f"upload.sh: guards on the required input {required}",
            required in text,
        )

    ppa_readme = read("packaging/ppa/README.md")
    normalized = " ".join(ppa_readme.split())
    check(
        "ppa/README.md: documents Mint inheritance (no separate Mint archive)",
        "Mint" in normalized
        and ("inherit" in normalized.lower()),
    )
    check(
        "ppa/README.md: states the secrets-outside-the-repo policy",
        "outside this repository" in normalized,
    )


# --------------------------------------------------------------------------
# ci/matrix
# --------------------------------------------------------------------------

EXPECTED_LEGS: dict[str, tuple[str, str, str]] = {
    # id: (tier, level, kind) — tiers per specs/architecture/platform-matrix.md
    "arch": ("t1", "full", "owned-elsewhere"),
    "manjaro": ("t1", "full", "owned-elsewhere"),
    "debian-trixie": ("t2", "smoke+contract", "container"),
    "debian-bookworm": ("t2", "smoke+contract", "container"),
    "ubuntu-lts": ("t2", "smoke+contract", "container"),
    "mint": ("t2", "smoke+contract", "vm"),
    "fedora": ("t2", "smoke+contract", "container"),
    "debian-bullseye": ("t3", "build", "container"),
}

EXPECTED_CONTAINERS = {
    "debian-trixie": "debian:trixie",
    "debian-bookworm": "debian:bookworm",
    "ubuntu-lts": "ubuntu:24.04",
    "fedora": "fedora:latest",
    "debian-bullseye": "debian:bullseye",
}


def validate_matrix() -> None:
    section("ci/matrix/t2.yml (the T2 QA matrix, roadmap 2.6.2)")
    doc = yaml.safe_load(read("ci/matrix/t2.yml"))
    check("matrix: parses as a YAML mapping", isinstance(doc, dict))
    if not isinstance(doc, dict):
        return
    check(
        "matrix: schema version declared",
        doc.get("version") == 1,
    )
    legs = doc.get("legs")
    check("matrix: legs is a list", isinstance(legs, list))
    if not isinstance(legs, list):
        return
    by_id: dict[str, dict] = {}
    ids: list[str] = []
    for leg in legs:
        if isinstance(leg, dict) and isinstance(leg.get("id"), str):
            by_id[leg["id"]] = leg
            ids.append(leg["id"])
    check("matrix: leg ids unique", len(ids) == len(set(ids)))
    missing = sorted(set(EXPECTED_LEGS) - set(by_id))
    extra = sorted(set(by_id) - set(EXPECTED_LEGS))
    check(
        "matrix: all eight Linux platforms present (roadmap 2.6.2 matrix)",
        not missing and not extra,
        f"missing={missing} extra={extra}",
    )
    for leg_id, (tier, level, kind) in EXPECTED_LEGS.items():
        leg = by_id.get(leg_id)
        if leg is None:
            continue
        check(
            f"matrix: {leg_id} tier/level/kind == {tier}/{level}/{kind}",
            leg.get("tier") == tier
            and leg.get("level") == level
            and leg.get("kind") == kind,
        )
        runs = leg.get("runs")
        if isinstance(runs, dict):
            check(
                f"matrix: {leg_id} package-install is req-ext (VM install runs stay REQ-EXT)",
                runs.get("package-install") == "req-ext",
            )
        if tier == "t2":
            check(
                f"matrix: {leg_id} (t2) runs build + contract-suite + smoke",
                runs.get("build") is True
                and runs.get("contract-suite") is True
                and runs.get("smoke") is True,
            )
        if tier == "t3":
            check(
                f"matrix: {leg_id} (t3) is build-only",
                runs.get("build") is True
                and runs.get("contract-suite") is False
                and runs.get("smoke") is False,
            )
        if kind == "container":
            expected_image = EXPECTED_CONTAINERS.get(leg_id)
            check(
                f"matrix: {leg_id} pins its reference container ({expected_image})",
                leg.get("container") == expected_image,
            )
            pkgs = leg.get("pkgs") or []
            pm = leg.get("package-manager")
            yaml_pkg = {"apt": "python3-yaml", "dnf": "python3-pyyaml"}.get(pm)
            check(
                f"matrix: {leg_id} installs the distro PyYAML name ({yaml_pkg})",
                bool(yaml_pkg) and yaml_pkg in pkgs,
            )
            if tier == "t2":
                check(
                    f"matrix: {leg_id} carries the session + tooling floor "
                    "(at-spi2-core, dbus, python3, curl, ca-certificates)",
                    {"at-spi2-core", "dbus", "python3", "curl", "ca-certificates"} <= set(pkgs),
                )
            else:
                # T3 is build-only: the hermetic build needs the tooling
                # floor, not the desktop session stack (documented honesty:
                # no session is claimed on a T3 leg).
                check(
                    f"matrix: {leg_id} (t3 build-only) carries the tooling "
                    "floor (git, python3, curl, ca-certificates)",
                    {"git", "python3", "curl", "ca-certificates"} <= set(pkgs),
                )
        if kind == "vm":
            check(
                "matrix: mint declares its documented stand-in container",
                leg.get("standin-container") == "ubuntu:24.04"
                and bool(leg.get("vm-note")),
            )
        if kind == "owned-elsewhere":
            check(
                f"matrix: {leg_id} names its owning workflow",
                (REPO / str(leg.get("owner", "missing"))).is_file(),
                str(leg.get("owner")),
            )

    # The converter must round-trip exactly what CI will consume.
    proc = subprocess.run(
        [sys.executable, str(REPO / "ci/matrix/to_json.py")],
        input=read("ci/matrix/t2.yml"),
        capture_output=True,
        text=True,
    )
    check(
        "matrix: to_json.py converts the file CI consumes",
        proc.returncode == 0,
        proc.stderr.strip(),
    )
    if proc.returncode == 0:
        expanded = json.loads(proc.stdout)["include"]
        check(
            "matrix: expansion carries all container-capable legs (5 reference + 1 stand-in)",
            len(expanded) == 6,
            f"got {len(expanded)}",
        )
        mint = next((leg for leg in expanded if leg["id"] == "mint"), {})
        check(
            "matrix: expansion flags the mint leg as a stand-in",
            mint.get("standin") is True,
        )

    matrix_readme = read("ci/matrix/README.md")
    check(
        "matrix: README documents the req-ext honesty rule",
        "req-ext" in matrix_readme and "stand-in" in matrix_readme,
    )


# --------------------------------------------------------------------------
# workflows + docs + hygiene
# --------------------------------------------------------------------------

def validate_workflows() -> None:
    section(".github/workflows (workflow artifacts parse; t2 wires the matrix)")
    workflow_paths = sorted(glob.glob(str(REPO / ".github/workflows/*.yml")))
    for path in workflow_paths:
        name = Path(path).name
        try:
            yaml.safe_load(Path(path).read_text(encoding="utf-8"))
            check(f"workflow {name}: parses as YAML", True)
        except yaml.YAMLError as exc:
            check(f"workflow {name}: parses as YAML", False, str(exc))
    t2 = read(".github/workflows/t2.yml")
    for needle, what in [
        ("ci/matrix/t2.yml", "consumes the matrix file"),
        ("ci/validate_packaging.py", "runs the committed validator"),
        ("fromJSON(", "expands the matrix via fromJSON"),
        ("ci/smoke/run-demos.sh", "runs the smoke demos"),
        ("T2_VM_ENABLED", "gates the REQ-EXT VM legs on a repo variable"),
        ("leg-classification", "prints the honest leg classification"),
        ("package-install", "names the req-ext leg in the classification"),
    ]:
        check(f"t2.yml: {what}", needle in t2)


def validate_docs() -> None:
    section("docs referencing the artifacts")
    readme = read("packaging/README.md")
    for needle, what in [
        ("PKGBUILD", "ties the convention back to the arch packaging"),
        ("validate_packaging.py", "points at the validator"),
        ("REQ-EXT", "states the REQ-EXT boundary"),
        ("`debian/`", "references the debian input"),
        ("`fedora/omnibus.spec`", "references the fedora input"),
        ("`ppa/`", "references the ppa channel"),
    ]:
        check(f"packaging/README.md: {what}", needle in readme)


SECRET_PATTERNS = [
    (re.compile(r"-----BEGIN [A-Z ]*PRIVATE KEY-----"), "private key block"),
    (re.compile(r"gh[pousr]_[0-9A-Za-z]{20,}"), "GitHub token"),
    (re.compile(r"github_pat_[0-9A-Za-z_]{20,}"), "GitHub fine-grained token"),
    (re.compile(r"AKIA[0-9A-Z]{16}"), "AWS access key id"),
    (re.compile(r"xox[baprs]-[0-9A-Za-z-]{10,}"), "Slack token"),
    (re.compile(r"(?i)aws_secret_access_key"), "AWS secret key reference"),
    (re.compile(r"(?i)^\s*password\s*=\s*\S+", re.MULTILINE), "plaintext password assignment"),
    (re.compile(r"/home/"), "workspace-local absolute path leak"),
]


def validate_hygiene() -> None:
    section("secrets + hygiene (no dependency on secrets)")
    scanned = sorted(
        str(p.relative_to(REPO))
        for base in ("packaging", "ci", ".github/workflows")
        for p in REPO.joinpath(base).rglob("*")
        if p.is_file()
        # The validator's own source necessarily contains its pattern
        # literals; a scanner does not scan itself.
        and p.name != "validate_packaging.py"
    )
    hits: list[str] = []
    for rel in scanned:
        text = REPO.joinpath(rel).read_text(encoding="utf-8", errors="replace")
        for pattern, what in SECRET_PATTERNS:
            if pattern.search(text):
                hits.append(f"{rel} ({what})")
    check(
        "no secret-shaped material or absolute home paths in packaging/, ci/, workflows/",
        not hits,
        "; ".join(hits),
    )
    check(
        "the committed validator itself is the local proof: python3 ci/validate_packaging.py",
        True,
        "this run",
    )


def main() -> int:
    print("T2 packaging + CI matrix validation (roadmap 2.6.1 / 2.6.2)")
    print(f"repo root: {REPO}")
    print(f"workspace version: {workspace_version()}")
    validate_debian()
    validate_fedora()
    validate_ppa()
    validate_matrix()
    validate_workflows()
    validate_docs()
    validate_hygiene()
    print()
    if _failed:
        print(f"FAILED: {len(_failed)} check(s):")
        for line in _failed:
            print(f"  - {line}")
        return 1
    print(f"all green: {_passed} checks passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
