#!/usr/bin/env bash
# What the windowed presentation spike needs installed on a machine of the
# research CI. On Linux: the Vulkan loader and Mesa's software device; a
# virtual X11 display and a Wayland compositor that needs no display, which
# the spike starts itself; and the headers SDL's build needs to hold its X11
# and Wayland video drivers, whose libraries it opens at run time. Windows
# and macOS need nothing: SDL is built from its sources by the tools their
# images hold. The research CI runs this script before the build; it prints
# what it did.
set -euo pipefail

case "$(uname -s)" in
  Linux*)
    if ! command -v apt-get >/dev/null 2>&1; then
      echo "setup: no apt-get on this Linux; the spike records what it finds"
      exit 0
    fi
    sudo=""
    if [ "$(id -u)" -ne 0 ]; then
      sudo="sudo"
    fi
    export DEBIAN_FRONTEND=noninteractive
    $sudo apt-get update -qq
    $sudo apt-get install -y -qq --no-install-recommends \
      libvulkan1 mesa-vulkan-drivers xvfb weston \
      build-essential cmake pkg-config \
      libx11-dev libxext-dev libxrandr-dev libxcursor-dev libxfixes-dev libxi-dev libxss-dev libxtst-dev \
      libxkbcommon-dev libxkbcommon-x11-0 libx11-xcb1 \
      libwayland-dev libwayland-bin wayland-protocols libegl-dev libgles-dev libdecor-0-dev \
      libasound2-dev libudev-dev libdbus-1-dev
    echo "setup: installed the Vulkan loader, Mesa's Vulkan drivers, Xvfb, weston and the headers of SDL's X11, Wayland and audio drivers"
    dpkg-query -W -f='${Package} ${Version}\n' libvulkan1 mesa-vulkan-drivers xvfb weston libwayland-dev libx11-dev 2>/dev/null || true
    ls /usr/share/vulkan/icd.d/ 2>/dev/null || true
    ;;
  *)
    echo "setup: nothing to install on $(uname -s)"
    ;;
esac
