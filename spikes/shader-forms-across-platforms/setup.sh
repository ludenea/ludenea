#!/usr/bin/env bash
# What the shading forms spike needs on a machine of the research CI: what
# the device baseline spike needs (a device, its loader and the glslang tool
# on Linux), which its own setup script installs, then the compilers the
# spike runs as tools, each taken from its published release and unpacked
# under target/shader-tools, where the spike looks for them:
#
# - Slang's compiler (Apache 2.0 license with the LLVM exception), for the
#   five cells;
# - the DirectX shader compiler (University of Illinois/NCSA license), which
#   Microsoft publishes for Windows on both architectures and for Linux on
#   x86_64 only;
# - the glslang tool (BSD, MIT and Apache 2.0 parts) for Windows and macOS,
#   the Linux cells taking the distribution's package.
#
# Apple's Metal compiler is part of Xcode on macOS; Apple gives its Metal
# developer tools for Windows behind a sign-in, a step no runner can take, so
# none is fetched there. A tool that cannot be fetched is said and the spike
# records the forms that need it as not made.
set -euo pipefail

bash "$(dirname "$0")/../render-device-baseline/setup.sh"

slang_release="https://github.com/shader-slang/slang/releases/download/v2026.22/slang-2026.22"
dxc_release="https://github.com/microsoft/DirectXShaderCompiler/releases/download/v1.9.2609"
glslang_release="https://github.com/KhronosGroup/glslang/releases/download/16.6.0/glslang-16.6.0"
tools="target/shader-tools"
mkdir -p "$tools"

# get <name> <url>: fetches an archive and unpacks it under $tools/<name>.
get() {
  local name="$1" url="$2"
  local archive="$tools/${url##*/}"
  if [ -d "$tools/$name" ]; then
    echo "setup: $name is already under $tools"
    return 0
  fi
  echo "setup: fetching $name from $url"
  mkdir -p "$tools/$name"
  if curl --proto '=https' --tlsv1.2 -sSfL --retry 3 -o "$archive" "$url"; then
    case "$archive" in
      *.zip)
        # unzip ends with status 1 on a warning, such as an archive whose
        # paths use backslashes, and has unpacked it all the same.
        unzip -q -o "$archive" -d "$tools/$name" || [ "$?" -eq 1 ] || rm -rf "$tools/$name"
        ;;
      *) tar -xzf "$archive" -C "$tools/$name" || rm -rf "$tools/$name" ;;
    esac
  else
    rm -rf "$tools/$name"
  fi
  rm -f "$archive"
  if [ ! -d "$tools/$name" ]; then
    echo "setup: $name could not be fetched or unpacked"
  fi
}

machine="$(uname -m)"
case "${PROCESSOR_ARCHITECTURE:-} ${RUNNER_ARCH:-}" in
  *ARM64*) machine="arm64" ;;
esac

case "$(uname -s)" in
  Linux*)
    case "$machine" in
      x86_64)
        get slang "$slang_release-linux-x86_64.tar.gz"
        get dxc "$dxc_release/linux_dxc_2026_09_28.x86_x64.tar.gz"
        ;;
      *)
        get slang "$slang_release-linux-aarch64.tar.gz"
        echo "setup: Microsoft publishes no DirectX shader compiler for Linux on this architecture"
        ;;
    esac
    ;;
  Darwin*)
    get slang "$slang_release-macos-aarch64.tar.gz"
    get glslang "$glslang_release-macos-universal-release.tar.gz"
    echo "setup: Microsoft publishes no DirectX shader compiler for macOS"
    xcrun -sdk macosx metal --version 2>&1 | head -n 2 || echo "setup: the Metal compiler of Xcode is unavailable (xcrun metal)"
    ;;
  MINGW*|MSYS*|CYGWIN*)
    case "$machine" in
      arm64) get slang "$slang_release-windows-aarch64.zip" ;;
      *) get slang "$slang_release-windows-x86_64.zip" ;;
    esac
    get dxc "$dxc_release/dxc_2026_09_29.zip"
    get glslang "$glslang_release-windows-x86_64-release.zip"
    echo "setup: Apple's Metal developer tools for Windows are not fetched: they are given behind a sign-in"
    ;;
esac

echo "setup: the tools under $tools:"
find "$tools" -maxdepth 4 -type f \( -name 'slangc*' -o -name 'dxc' -o -name 'dxc.exe' -o -name 'glslang' -o -name 'glslang.exe' -o -name 'glslangValidator*' -o -name 'libdxcompiler*' -o -name 'dxcompiler.dll' -o -name 'dxil.dll' -o -name 'libdxil*' \) | sort
