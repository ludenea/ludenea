#!/usr/bin/env bash
# What the backend comparison spike needs installed on a machine of the
# research CI: what the device baseline spike needs (the Vulkan loader,
# Mesa's software device, the GLSL compiler and the Vulkan validation layer
# on Linux; the optional component that holds Direct3D 12's validation layer
# on Windows), which its own setup script installs. On a cell whose label
# contains "no-loader" nothing is installed: that cell measures whether a
# binary that links wgpu starts on a machine with no Vulkan loader. The
# script then prints the shader compilers the spike will run as tools.
set -euo pipefail

case "${CELL:-}" in
  *no-loader*)
    echo "setup: the cell is labelled no-loader, nothing is installed"
    exit 0
    ;;
esac

bash "$(dirname "$0")/../render-device-baseline/setup.sh"

case "$(uname -s)" in
  Linux*)
    (glslangValidator --version 2>/dev/null || glslang --version 2>/dev/null || echo "setup: no GLSL compiler on the path") | head -n 1
    ;;
  Darwin*)
    xcrun -sdk macosx metal --version 2>&1 | head -n 2 || echo "setup: the Metal compiler of Xcode is unavailable (xcrun metal)"
    ;;
  MINGW*|MSYS*|CYGWIN*)
    ls "/c/Program Files (x86)/Windows Kits/10/bin/"*/*/dxc.exe 2>/dev/null || echo "setup: no dxc.exe in the Windows SDK"
    ;;
esac
