#!/usr/bin/env bash
# Builds the Box2D half of the physics snapshot spike, a C program, with CMake,
# as the research CI builds a C spike: release mode, the platform's default
# compiler, or the one named by CC and CXX. One rule is added: on Windows for
# ARM64 the compiler is Clang, since foreign code inside the deterministic
# domain is built with GCC or Clang on every cell and with MSVC on x86_64
# only; Box2D v3.2.0 as released needs two flags to compile there (see the
# Box2D worker sweep spike's README). The research CI runs this script from
# spikes/ before the Cargo build; the Rust program then runs the executable.
set -euo pipefail

name=physics-snapshot-every-tick-box2d
source_dir="$(dirname "$0")/box2d"
build="build/$name"

generator=()
case "$(uname -s)" in
  MINGW*|MSYS*|CYGWIN*)
    if [ -z "${CC:-}" ] && [ "${PROCESSOR_ARCHITECTURE:-}" = "ARM64" ]; then
      export CC='clang-cl /FIintrin.h /DARM64_FPCR=0x5A20'
      export CXX="$CC"
      echo "setup: Windows for ARM64, building the Box2D half with $CC"
    fi
    if [ -n "${CC:-}" ]; then generator=(-G Ninja); fi
    ;;
esac

cmake -S "$source_dir" -B "$build" "${generator[@]}" -DCMAKE_BUILD_TYPE=Release
cmake --build "$build" --config Release --parallel
cmake --install "$build" --config Release --prefix "$build/install" >/dev/null
echo "--- compiler of the Box2D half"
grep -h -E '^set\(CMAKE_C_COMPILER(_ID|_VERSION|_FRONTEND_VARIANT)? ' "$build"/CMakeFiles/*/CMakeCCompiler.cmake || true
echo "setup: built $build/install/bin/$name"
