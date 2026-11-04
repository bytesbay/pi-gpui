#!/bin/sh
# Publishes the version in Cargo.toml: tags it and pushes the tag. GitHub
# Actions (.github/workflows/release.yml) builds the app and creates the
# release; running apps on the same pi branch then offer "Restart to update".
#
# Bump `version` in Cargo.toml first (semver of the app itself; supported pi
# versions live in src/pi.rs), commit, then run this script.
set -eu
cd "$(dirname "$0")/.."
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
if [ -n "$(git status --porcelain)" ]; then
    echo "Commit your changes first." >&2
    exit 1
fi
if git rev-parse "v$version" >/dev/null 2>&1; then
    echo "v$version is already tagged; bump the version in Cargo.toml." >&2
    exit 1
fi
git tag -a "v$version" -m "Pi GPUI $version"
git push origin HEAD "v$version"
echo "Pushed v$version. Follow the build: https://github.com/bytesbay/pi-gpui/actions"
