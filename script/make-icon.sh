#!/bin/sh
# Regenerates resources/AppIcon.icns from resources/AppIcon.svg.
# With a mark SVG as argument, first rebuilds AppIcon.svg from it:
#   script/make-icon.sh path/to/pi-mark.svg
# Needs rsvg-convert (`brew install librsvg`); iconutil ships with macOS.
# The .icns is committed, so building the app does not need this script.
set -eu
mark=""
if [ $# -gt 0 ]; then
    mark=$(cd "$(dirname "$1")" && pwd)/$(basename "$1")
fi
cd "$(dirname "$0")/.."
if [ -n "$mark" ]; then
    python3 script/icon_from_mark.py "$mark"
fi
set_dir=$(mktemp -d)/AppIcon.iconset
mkdir -p "$set_dir"
for size in 16 32 128 256 512; do
    rsvg-convert -w "$size" -h "$size" resources/AppIcon.svg -o "$set_dir/icon_${size}x${size}.png"
    double=$((size * 2))
    rsvg-convert -w "$double" -h "$double" resources/AppIcon.svg -o "$set_dir/icon_${size}x${size}@2x.png"
done
iconutil -c icns "$set_dir" -o resources/AppIcon.icns
rm -rf "$(dirname "$set_dir")"
echo resources/AppIcon.icns
