#!/usr/bin/env python3
"""Convert ci/matrix/t2.yml into the JSON a GitHub Actions matrix consumes.

.github/workflows/t2.yml runs this on the host runner and feeds the output
to `strategy.matrix` via fromJSON: every `kind: container` leg becomes one
job; `kind: vm` legs are the documented stand-ins (standin-* fields); the
`kind: owned-elsewhere` T1 legs are informational rows only.

The JSON carries exactly the fields the workflow steps branch on — the
matrix file stays the single source of truth, the workflow stays generic.

Reads stdin, writes JSON to stdout (errors to stderr, exit 1).
"""
from __future__ import annotations

import json
import sys

import yaml


def main() -> int:
    try:
        doc = yaml.safe_load(sys.stdin.read())
    except yaml.YAMLError as exc:
        print(f"error: ci/matrix/t2.yml does not parse: {exc}", file=sys.stderr)
        return 1

    if not isinstance(doc, dict) or not isinstance(doc.get("legs"), list):
        print("error: expected a mapping with a 'legs' list", file=sys.stderr)
        return 1

    container_legs: list[dict[str, object]] = []
    for leg in doc["legs"]:
        if not isinstance(leg, dict):
            print("error: each leg must be a mapping", file=sys.stderr)
            return 1
        if leg.get("kind") == "owned-elsewhere":
            continue
        # A vm leg runs its documented stand-in container in CI; the real VM
        # execution stays REQ-EXT (manual dispatch + runners, never faked).
        container = leg.get("container") or leg.get("standin-container")
        if container is None:
            print(f"error: leg {leg.get('id')!r}: no container to run", file=sys.stderr)
            return 1
        container_legs.append(
            {
                "id": leg["id"],
                "container": container,
                "package-manager": leg.get("package-manager")
                or leg.get("standin-package-manager"),
                "pkgs": leg.get("pkgs") or leg.get("standin-pkgs") or [],
                "runs": leg.get("runs", {}),
                "standin": leg.get("kind") == "vm",
            }
        )

    json.dump({"include": container_legs}, sys.stdout)
    sys.stdout.write("\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
