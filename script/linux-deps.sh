#!/bin/sh
# Installs what building Pi GPUI needs on Debian and Ubuntu. Other
# distributions: the same libraries under their own package names (see
# "Linux" in README.md).
set -eu
export DEBIAN_FRONTEND=noninteractive
packages="build-essential clang cmake pkg-config curl git
    libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev
    libx11-xcb-dev libxcb1-dev libfontconfig-dev libfreetype-dev
    libssl-dev libzstd-dev libvulkan-dev libasound2-dev"

apt_get() {
    sudo -E apt-get -o Acquire::Retries=1 -o Acquire::http::Timeout=15 \
        -o Acquire::https::Timeout=15 "$@"
}

install() {
    # shellcheck disable=SC2086
    apt_get -o APT::Update::Error-Mode=any update -q &&
        apt_get install -y -q --no-install-recommends $packages
}

if install; then
    exit 0
fi
# Only CI gets its package sources rewritten.
[ -n "${CI:-}" ] || exit 1

# GitHub's runners now and then cannot reach their default Ubuntu mirror
# (the names do not resolve for an hour or two). Other mirrors still work.
. /etc/os-release
case "$(dpkg --print-architecture)" in
    amd64) path=ubuntu ;;
    *) path=ubuntu-ports ;;
esac
for mirror in https://mirrors.ocf.berkeley.edu https://mirrors.mit.edu; do
    echo "The default mirror failed; trying $mirror"
    for suite in "" -updates -security; do
        echo "deb $mirror/$path $VERSION_CODENAME$suite main universe"
    done | sudo tee /etc/apt/sources.list >/dev/null
    sudo rm -f /etc/apt/sources.list.d/ubuntu.sources
    if install; then
        exit 0
    fi
done
exit 1
