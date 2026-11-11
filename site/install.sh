#!/bin/sh
# Installs Pi GPUI on Linux (x86_64 and aarch64):
#
#   curl -fsSL https://pi-gpui.dev/install.sh | sh
#
# It downloads the newest release from GitHub into ~/.local/pi-gpui.app, links
# ~/.local/bin/pi-gpui and adds the app to your launcher. Nothing needs root.
# The app then keeps itself up to date. To remove it:
#
#   rm -rf ~/.local/pi-gpui.app ~/.local/bin/pi-gpui \
#          ~/.local/share/applications/pi-gpui.desktop
#
# Options (environment variables):
#   PI_GPUI_VERSION=0.2.0   install this version instead of the newest
#   PI_GPUI_PREFIX=~/.local where pi-gpui.app and bin/ go
#   PI_GPUI_ARCHIVE=path    install from a local tarball (no download)
set -eu

REPOSITORY="bytesbay/pi-gpui"

fail() {
    echo "pi-gpui: $1" >&2
    exit 1
}

need() {
    command -v "$1" >/dev/null 2>&1 || fail "'$1' is required; install it and run this again"
}

main() {
    if [ "$(uname -s)" != "Linux" ]; then
        fail "this installer is for Linux; on macOS run: brew install --cask bytesbay/tap/pi-gpui"
    fi
    case "$(uname -m)" in
        x86_64 | amd64) arch=x86_64 ;;
        aarch64 | arm64) arch=aarch64 ;;
        *) fail "no build for $(uname -m) yet; see https://github.com/$REPOSITORY" ;;
    esac
    need tar

    prefix=${PI_GPUI_PREFIX:-$HOME/.local}
    data=${XDG_DATA_HOME:-$HOME/.local/share}
    mkdir -p "$prefix/bin" "$data/applications"
    work=$(mktemp -d "$prefix/.pi-gpui-install.XXXXXX")
    trap 'rm -rf "$work"' EXIT

    if [ -n "${PI_GPUI_ARCHIVE:-}" ]; then
        archive=$PI_GPUI_ARCHIVE
    else
        need curl
        want=${PI_GPUI_VERSION:+/v$PI_GPUI_VERSION/}
        # Releases come newest first; take the first one with a build for this machine.
        url=$(curl -fsSL "https://api.github.com/repos/$REPOSITORY/releases?per_page=30" |
            grep -o "https://[^\"]*${want:-}[^\"]*-linux-$arch\.tar\.gz" | head -1)
        [ -n "$url" ] || fail "no Linux build found${PI_GPUI_VERSION:+ for version $PI_GPUI_VERSION} (or GitHub did not answer); see https://github.com/$REPOSITORY/releases"
        echo "Downloading $url"
        archive="$work/pi-gpui.tar.gz"
        curl -fL --progress-bar -o "$archive" "$url" || fail "the download failed"
    fi

    tar -xzf "$archive" -C "$work" || fail "the archive could not be unpacked"
    [ -x "$work/pi-gpui.app/bin/pi-gpui" ] || fail "the archive has no pi-gpui.app inside"
    version=$("$work/pi-gpui.app/bin/pi-gpui" --version 2>/dev/null) ||
        fail "the app does not start on this system (it needs glibc 2.35 or newer)"

    app="$prefix/pi-gpui.app"
    rm -rf "$app"
    mv "$work/pi-gpui.app" "$app"
    ln -sf "$app/bin/pi-gpui" "$prefix/bin/pi-gpui"

    # The launcher entry points at this install, so it works without
    # ~/.local/bin on PATH and without an icon theme cache.
    sed -e "s|^Exec=.*|Exec=\"$app/bin/pi-gpui\" %u|" \
        -e "s|^Icon=.*|Icon=$app/share/icons/hicolor/512x512/apps/pi-gpui.png|" \
        "$app/share/applications/pi-gpui.desktop" >"$data/applications/pi-gpui.desktop"
    if command -v update-desktop-database >/dev/null 2>&1; then
        update-desktop-database "$data/applications" >/dev/null 2>&1 || true
    fi
    # pi-gpui:// links (the /gpui command of the pi-gpui pi package).
    if command -v xdg-mime >/dev/null 2>&1; then
        xdg-mime default pi-gpui.desktop x-scheme-handler/pi-gpui >/dev/null 2>&1 || true
    fi

    echo "Installed $version in $app"
    case ":$PATH:" in
        *":$prefix/bin:"*) echo "Run it from your launcher or with: pi-gpui" ;;
        *) echo "Run it from your launcher or with: $prefix/bin/pi-gpui" ;;
    esac
    if ! command -v pi >/dev/null 2>&1; then
        echo "pi itself is not on PATH yet; the app shows how to install it on first launch."
    fi
}

main "$@"
