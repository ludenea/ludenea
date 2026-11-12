#!/usr/bin/env bash
# What the visual reference round trip spike needs installed on a machine of
# the research CI. On Linux: what the device baseline spike's native path
# needs (the Vulkan loader, Mesa's Vulkan drivers and the GLSL compiler), and
# Netpbm, whose pngtopam is the independent PNG decoder and whose pamtopng
# writes a picture again. On macOS: Netpbm, from Homebrew. On Windows nothing:
# the decoder is the PNG codec of GDI+, which ships with the system. The
# research CI runs this script before the build; it prints what it did.
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
    $sudo apt-get install -y -qq --no-install-recommends libvulkan1 mesa-vulkan-drivers glslang-tools netpbm git
    echo "setup: installed libvulkan1, mesa-vulkan-drivers, glslang-tools, netpbm and git"
    dpkg-query -W -f='${Package} ${Version}\n' libvulkan1 mesa-vulkan-drivers glslang-tools netpbm libpng16-16 libpng16-16t64 git 2>/dev/null || true
    ;;
  Darwin*)
    if command -v pngtopam >/dev/null 2>&1 && command -v pamtopng >/dev/null 2>&1; then
      echo "setup: Netpbm is present"
    else
      HOMEBREW_NO_AUTO_UPDATE=1 HOMEBREW_NO_INSTALL_CLEANUP=1 brew install --quiet netpbm
      echo "setup: installed Netpbm from Homebrew"
    fi
    brew list --versions netpbm libpng 2>/dev/null || true
    ;;
  MINGW*|MSYS*|CYGWIN*)
    powershell.exe -NoProfile -Command \
      '"setup: Windows PowerShell " + $PSVersionTable.PSVersion + "; System.Drawing " + ([System.Reflection.Assembly]::LoadWithPartialName("System.Drawing")).GetName().Version' \
      | tr -d '\r' || true
    ;;
  *)
    echo "setup: nothing to install on $(uname -s)"
    ;;
esac
