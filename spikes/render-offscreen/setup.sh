#!/usr/bin/env bash
# What the offscreen rendering spike needs installed on a Linux machine: the
# Vulkan loader, Mesa's Vulkan drivers (lavapipe, the software device) and the
# GLSL compiler that produces the SPIR-V at run time. Windows and macOS need
# nothing: the WARP device and the Metal device ship with the system. The
# research CI runs this script before the build; it prints what it did.
set -euo pipefail

case "$(uname -s)" in
  Linux*)
    if ! command -v apt-get >/dev/null 2>&1; then
      echo "setup: no apt-get on this Linux; the Vulkan path records what it finds"
      exit 0
    fi
    sudo=""
    if [ "$(id -u)" -ne 0 ]; then
      sudo="sudo"
    fi
    export DEBIAN_FRONTEND=noninteractive
    $sudo apt-get update -qq
    $sudo apt-get install -y -qq --no-install-recommends libvulkan1 mesa-vulkan-drivers glslang-tools
    echo "setup: installed libvulkan1, mesa-vulkan-drivers and glslang-tools"
    dpkg-query -W -f='${Package} ${Version}\n' libvulkan1 mesa-vulkan-drivers glslang-tools 2>/dev/null || true
    ls /usr/share/vulkan/icd.d/ 2>/dev/null || true
    ;;
  *)
    echo "setup: nothing to install on $(uname -s)"
    ;;
esac
