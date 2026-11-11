#!/bin/sh
# Builds the Linux release in target/release:
#   pi-gpui.app/                       the install folder (the updater swaps it as a whole)
#     bin/pi-gpui
#     share/applications/pi-gpui.desktop
#     share/icons/hicolor/512x512/apps/pi-gpui.png
#     share/pi-gpui/                   built-in plugins, licenses
#   pi-gpui-<version>-linux-<arch>.tar.gz
# site/install.sh unpacks the tarball into ~/.local and registers the app.
set -eu
cd "$(dirname "$0")/.."
cargo build --release --locked
target=${CARGO_TARGET_DIR:-target}/release
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
arch=$(uname -m)
app="$target/pi-gpui.app"
rm -rf "$app"
mkdir -p "$app/bin" "$app/share/applications" \
    "$app/share/icons/hicolor/512x512/apps" "$app/share/pi-gpui"
cp "$target/pi-gpui" "$app/bin/pi-gpui"
strip "$app/bin/pi-gpui"
cp resources/linux/pi-gpui.desktop "$app/share/applications/pi-gpui.desktop"
cp resources/linux/pi-gpui.png "$app/share/icons/hicolor/512x512/apps/pi-gpui.png"
if [ -d plugins ]; then
    cp -R plugins "$app/share/pi-gpui/plugins"
fi
cp LICENSE "$app/share/pi-gpui/LICENSE"
# The bundled JetBrains Mono font is distributed under this license.
cp assets/fonts/OFL.txt "$app/share/pi-gpui/JetBrainsMono-OFL.txt"
asset="$target/pi-gpui-$version-linux-$arch.tar.gz"
tar -czf "$asset" -C "$target" pi-gpui.app
echo "$asset"
