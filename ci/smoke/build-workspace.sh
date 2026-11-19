# Shared build step for the smoke demos (sourced, not executed).
#
# Arch-family machines (the ci T1 legs, the manjaro legs and any developer
# machine — libalpm ships with pacman there) build the whole workspace, so
# demo3 exercises the real alpm backend.
#
# Everywhere else the cross-family build contract applies (ci/matrix/t2.yml):
# the alpm adapter links the system libalpm, which is absent on non-Arch
# distros by design — their package backend is apt/dnf — so it is excluded
# exactly as the ci.yml and t2.yml build/test steps exclude it:
# `--no-default-features --exclude omnibus-adapter-alpm`.
#
# Detecting libalpm through pkg-config mirrors how alpm-sys itself finds it
# (LIBALPM_* env overrides); any non-zero probe answer routes to the
# cross-family branch, so a missing pkg-config binary degrades safely too.

if pkg-config --exists libalpm 2>/dev/null; then
  cargo build --workspace --quiet
else
  cargo build --workspace --no-default-features --exclude omnibus-adapter-alpm --quiet
fi
