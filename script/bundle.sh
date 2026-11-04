#!/bin/sh
# Builds "Pi GPUI.app" in target/release. Run it with `open` so macOS treats
# the app as responsible for its plugins: permission prompts (microphone,
# speech recognition) then name the app and use the descriptions in its
# Info.plist. A bare binary started from a terminal inherits the terminal's.
set -eu
cd "$(dirname "$0")/.."
cargo build --release
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
app="target/release/Pi GPUI.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp target/release/pi-gpui "$app/Contents/MacOS/pi-gpui"
# Built-in plugins travel inside the bundle, so the app never reads the
# source tree (which may live in a folder macOS guards, like ~/Documents).
if [ -d plugins ]; then
    cp -R plugins "$app/Contents/Resources/plugins"
fi
sed "s/__VERSION__/$version/g" resources/Info.plist > "$app/Contents/Info.plist"
cp resources/AppIcon.icns "$app/Contents/Resources/AppIcon.icns"
codesign --force --sign - "$app"
echo "$app"
