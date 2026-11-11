#!/bin/sh
# Installs what building Pi GPUI needs on Debian and Ubuntu. Other
# distributions: the same libraries under their own package names (see
# "Linux" in README.md).
set -eu
sudo apt-get update -q
sudo apt-get install -y -q --no-install-recommends \
    build-essential clang cmake pkg-config curl git \
    libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev \
    libx11-xcb-dev libxcb1-dev libfontconfig-dev libfreetype-dev \
    libssl-dev libzstd-dev libvulkan-dev libasound2-dev
