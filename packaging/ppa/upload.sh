#!/usr/bin/env bash
# PPA source-package upload for the Ubuntu LTS channel (roadmap task 2.6.1).
#
# Cuts a signed Debian source package from the repository tree and dputs it
# to the Omnibus PPA. Linux Mint 22.x inherits the Ubuntu 24.04 base, so
# this one channel covers both distros (see packaging/ppa/README.md).
#
# Usage:  packaging/ppa/upload.sh <series-suffix>
#         packaging/ppa/upload.sh '~ubuntu24.04'
#
# The suffix is appended to the changelog version for this upload only
# (~deb12 / ~deb13 / ~ubuntu24.04); the committed changelog stays at the
# bare workspace version (1.5.0) - the suffix is channel metadata, never
# repo state.
#
# SECRETS POLICY (validated by ci/validate_packaging.py): signing happens
# with the maintainer's GnuPG key from the LOCAL keyring and Launchpad
# credentials from the uploader's OWN dput config outside this repository.
# This script contains no keys, tokens or passwords and CI never runs it -
# uploads are a human-issued, out-of-band action (the never-list).

set -euo pipefail

usage() {
	echo "usage: $0 <series-suffix>   e.g. $0 '~ubuntu24.04'" >&2
	exit 2
}

[ $# -eq 1 ] || usage
suffix=$1

# The suffix lands inside a dpkg version string AND inside a sed replacement,
# so the FULL string is constrained before it goes anywhere near either:
# '~' + [A-Za-z0-9.+-] only. A POSIX `case` pattern cannot express "the rest
# of the string" (its trailing `*` is a wildcard, so `~[class]*` would
# constrain the first character only — `~$(reboot)` would pass); the anchored
# bash regex does. The validation keeps `$`, quotes, backticks, `/`, `&` and
# `\` out of the double-quoted sed program and the dpkg version alike.
if [[ ! $suffix =~ ^~[A-Za-z0-9.+-]+$ ]]; then
	echo "error: suffix must start with '~' and use only [A-Za-z0-9.+-], got: $suffix" >&2
	exit 2
fi

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$REPO_ROOT"

for f in packaging/debian/control packaging/debian/rules packaging/debian/install packaging/debian/changelog packaging/ppa/dput.cf; do
	[ -f "$f" ] || { echo "error: missing $f - the debian/ packaging input and dput config are required" >&2; exit 1; }
done

workdir=$(mktemp -d)
trap 'rm -rf "$workdir"' EXIT

# Clean source tree: tracked files only (no .git, no target/ build tree). The
# tree unpacks into $workdir/src so the dpkg-buildpackage artifacts (.dsc,
# *_source.changes, .buildinfo, the source tarball) land in its PARENT —
# $workdir — where the lookup below and the EXIT trap both work:
# dpkg-buildpackage writes to the parent of the source tree it runs in,
# never into the tree itself.
mkdir "$workdir/src"
git archive --format=tar HEAD | tar -x -C "$workdir/src"

# Vendor the crates.io dependencies INTO the source package: Launchpad's
# builders are network-isolated, so the binary-deb build there cannot fetch
# the lockfile's crates. `cargo vendor` materializes every locked crate under
# vendor/ and emits the source-replacement config; debian/rules builds
# --offline against it. Runs on the release host (which has network), never
# on the builder.
mkdir -p "$workdir/src/.cargo"
# --locked: vendor EXACTLY the committed lockfile (the release host must not
# silently re-resolve; a drifted lock fails the cut loudly instead).
(cd "$workdir/src" && cargo vendor --locked vendor > .cargo/config.toml)

# Apply the series suffix to the changelog version of this upload only.
sed -i "1s/^omnibus (\([^)]*\))/omnibus (\1$suffix)/" "$workdir/src/packaging/debian/changelog"
head -1 "$workdir/src/packaging/debian/changelog"

# Move the debian/ dir into the position the Debian tooling expects, then
# build the source package. dpkg-buildpackage -S signs with the local
# default GnuPG key; allow_unsigned_uploads = 0 in dput.cf refuses to send
# a package that did not sign.
mv "$workdir/src/packaging/debian" "$workdir/src/debian"
cd "$workdir/src"
dpkg-buildpackage -S

# Exactly one .changes file, found in the artifact parent ($workdir): with
# nullglob an unmatched glob expands to NOTHING (and the count check dies
# loudly), never to a literal `omnibus_*.changes` string silently tested by
# [ -f ].
shopt -s nullglob
set -- "$workdir"/omnibus_*.changes
shopt -u nullglob
if [ "$#" -ne 1 ]; then
	echo "error: dpkg-buildpackage produced no single .changes file in $workdir" >&2
	exit 1
fi
changes=$1

# Host/incoming names come from packaging/ppa/dput.cf (see the placeholders
# note there); the local ~/.dput.cf may override or supply the real ones.
dput omnibus "$changes"
