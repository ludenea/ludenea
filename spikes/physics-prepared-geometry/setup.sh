#!/usr/bin/env bash
# Builds the Box2D half of the prepared collision geometry spike with CMake,
# before the Cargo build of the Rust half that runs it as a child process.
#
# Run by ci/run-spike.sh from the spikes directory. Builds box2d/ into
# build/physics-prepared-geometry-box2d/ and installs the executable under
# its install/bin/, exactly as the run script builds a C spike. Three
# additions:
#
# - SPIKE_PROFILE=dev builds the Debug configuration into
#   build/physics-prepared-geometry-box2d-debug/, so that the optimized and
#   the unoptimized builds coexist; unset, or any other value, means Release.
# - On Windows, a compiler named by CC or CXX builds with the Ninja
#   generator, because the Visual Studio generator ignores them.
# - On Windows for ARM64, with no compiler named, the build uses clang-cl
#   with the two flags Box2D v3.2.0 needs to compile there: the engine's
#   determinism contract admits MSVC for foreign code on x86_64 only (see
#   the Windows ARM64 section of ../box2d-worker-sweep/README.md).
set -euo pipefail

cd "$(dirname "$0")/.."

source_dir="physics-prepared-geometry/box2d"
if [ "${SPIKE_PROFILE:-release}" = dev ]; then
  config=Debug
  build="build/physics-prepared-geometry-box2d-debug"
else
  config=Release
  build="build/physics-prepared-geometry-box2d"
fi

generator=()
case "$(uname -s)" in
  MINGW*|MSYS*|CYGWIN*)
    # A shell that runs under emulation reports the emulated architecture;
    # the processor identifier and the runner's own variable name the real one.
    machine="$(uname -m)"
    case "${PROCESSOR_ARCHITECTURE:-} ${PROCESSOR_ARCHITEW6432:-} ${RUNNER_ARCH:-} ${PROCESSOR_IDENTIFIER:-}" in
      *ARM64*|*ARMv8*) machine="arm64" ;;
    esac
    case "$machine" in
      arm64|aarch64)
        if [ -z "${CC:-}" ]; then
          export CC='clang-cl /FIintrin.h /DARM64_FPCR=0x5A20'
          export CXX="$CC"
          echo "setup: Windows on ARM64 with no compiler named: CC and CXX set to $CC"
        fi
        ;;
    esac
    if [ -n "${CC:-}" ] || [ -n "${CXX:-}" ]; then
      generator=(-G Ninja)
    fi
    ;;
esac

echo "setup: building $source_dir ($config) into $build"
# ${array[@]+...} expands to nothing for an empty array under set -u on every
# bash, the one macOS ships included.
cmake -S "$source_dir" -B "$build" ${generator[@]+"${generator[@]}"} -DCMAKE_BUILD_TYPE="$config"
cmake --build "$build" --config "$config" --parallel
cmake --install "$build" --config "$config" --prefix "$build/install" >/dev/null

echo "setup: --- compiler"
grep -h -E '^set\(CMAKE_C(XX)?_COMPILER(_ID|_VERSION|_FRONTEND_VARIANT)? ' "$build"/CMakeFiles/*/CMakeC*Compiler.cmake || true
echo "setup: cc=${CC:-} cxx=${CXX:-}"

exe="$build/install/bin/physics-prepared-geometry-box2d"
if [ ! -x "$exe" ] && [ -x "$exe.exe" ]; then
  exe="$exe.exe"
fi
echo "setup: installed $exe"
