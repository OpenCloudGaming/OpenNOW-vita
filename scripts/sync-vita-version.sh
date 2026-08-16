#!/bin/sh
# Keeps [package.metadata.vita] vita_mksfoex_flags' APP_VER in sync with [package].version, so the
# Vita bubble's version string can't drift from Cargo's the way it did before (Cargo said 0.3.2,
# the VPK said 00.31). Vita APP_VER is XX.YY, not semver - this maps 0.MINOR.PATCH to 00.MINORPATCH
# (single-digit minor/patch only, matching every version this project has shipped so far).
set -eu

cd "$(dirname "$0")/.."

version=$(sed -n 's/^version = "\([^"]*\)".*/\1/p' Cargo.toml | head -1)
major=$(echo "$version" | cut -d. -f1)
minor=$(echo "$version" | cut -d. -f2)
patch=$(echo "$version" | cut -d. -f3)

case "$minor" in ''|*[!0-9]*) echo "sync-vita-version: bad minor '$minor' in version '$version'" >&2; exit 1 ;; esac
case "$patch" in ''|*[!0-9]*) echo "sync-vita-version: bad patch '$patch' in version '$version'" >&2; exit 1 ;; esac
if [ "$minor" -gt 9 ] || [ "$patch" -gt 9 ]; then
    echo "sync-vita-version: minor/patch must be single digits for the XX.YY APP_VER scheme (got $version)" >&2
    exit 1
fi

app_ver=$(printf '00.%d%d' "$minor" "$patch")

current=$(sed -n 's/.*APP_VER=\([0-9.]*\).*/\1/p' Cargo.toml | head -1)
if [ "$current" = "$app_ver" ]; then
    exit 0
fi

sed -i.bak "s/APP_VER=$current/APP_VER=$app_ver/" Cargo.toml
rm -f Cargo.toml.bak
echo "sync-vita-version: APP_VER $current -> $app_ver (from version = \"$version\")"
