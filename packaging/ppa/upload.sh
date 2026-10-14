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
# bare workspace version (0.1.0) - the suffix is channel metadata, never
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

# The suffix lands inside a dpkg version string; constrain it before it goes
# anywhere near sed or dpkg. Debian versions allow [A-Za-z0-9.+:~-], the
# ~ marks a backport-style ordering bump; keep the suffix to that subset.
case $suffix in
~[A-Za-z0-9.+-]*) ;;
*) echo "error: suffix must start with '~' and use only [A-Za-z0-9.+-], got: $suffix" >&2; exit 2 ;;
esac

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$REPO_ROOT"

for f in packaging/debian/control packaging/debian/rules packaging/debian/install packaging/debian/changelog packaging/ppa/dput.cf; do
	[ -f "$f" ] || { echo "error: missing $f - the debian/ packaging input and dput config are required" >&2; exit 1; }
done

workdir=$(mktemp -d)
trap 'rm -rf "$workdir"' EXIT

# Clean source tree: tracked files only (no .git, no target/ build tree).
git archive --format=tar HEAD | tar -x -C "$workdir"

# Apply the series suffix to the changelog version of this upload only.
sed -i "1s/^omnibus (\([^)]*\))/omnibus (\1$suffix)/" "$workdir/packaging/debian/changelog"
head -1 "$workdir/packaging/debian/changelog"

# Move the debian/ dir into the position the Debian tooling expects, then
# build the source package. dpkg-buildpackage -S signs with the local
# default GnuPG key; allow_unsigned_uploads = 0 in dput.cf refuses to send
# a package that did not sign.
mv "$workdir/packaging/debian" "$workdir/debian"
cd "$workdir"
dpkg-buildpackage -S

changes=$(echo omnibus_*.changes)
[ -f "$changes" ] || { echo "error: dpkg-buildpackage produced no .changes file" >&2; exit 1; }

# Host/incoming names come from packaging/ppa/dput.cf (see the placeholders
# note there); the local ~/.dput.cf may override or supply the real ones.
dput omnibus "$changes"
