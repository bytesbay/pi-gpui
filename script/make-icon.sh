#!/bin/sh
# Regenerates resources/AppIcon.icns and the website icons in site/img from resources/AppIcon.svg.
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

# Website icons. The app icon keeps the macOS margin around its plate (824 of 1024 px); browsers
# and home screens expect the artwork to fill the canvas, so the same icon there looks ~20%
# smaller than its neighbours. Crop to the plate instead.
plate='viewBox="100 100 824 824"'
sed "1s/viewBox=\"0 0 1024 1024\"/$plate/" resources/AppIcon.svg > site/img/icon.svg
rsvg-convert -w 64 -h 64 site/img/icon.svg -o site/img/favicon-64.png
# apple-touch-icon: opaque square, no corners, shadow or rim. The system applies its own mask.
touch_svg=$(mktemp).svg
sed -e 's/ rx="185"//' -e '/app-plate-shadow)"\/>/d' -e '/stroke="url(#app-rim)"/d' site/img/icon.svg > "$touch_svg"
rsvg-convert -w 180 -h 180 "$touch_svg" -o site/img/apple-touch-icon.png
rm -f "$touch_svg"
echo site/img/icon.svg site/img/favicon-64.png site/img/apple-touch-icon.png
