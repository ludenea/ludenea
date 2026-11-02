#!/usr/bin/env bash
# Builds one spike, runs it, and writes its measurements and the facts about
# the machine under out/<cell>/.
#
# Usage: ci/run-spike.sh <spike directory> <cell>
#   <spike directory>  a spike under spikes/, for example platform-facts
#   <cell>             a label for the machine, for example linux-x86_64
#
# A Rust spike is a member of the Cargo workspace (Cargo.toml in its directory)
# and is built with the pinned toolchain; RUSTFLAGS, when set, is passed to the
# compiler by Cargo and recorded in the environment file. A C spike has a
# CMakeLists.txt in its directory and is built in release mode with the
# platform's default compiler; its executable is named after the CMake project.
# Either way the spike prints one JSON document on standard output, which is
# saved as facts.json.
#
# Portable on purpose, so that the research CI never depends on GitHub Actions
# features: it runs unchanged on Linux, macOS and Windows (Git Bash), hosted or local.
set -euo pipefail

spike="${1:?usage: run-spike.sh <spike directory> <cell>}"
cell="${2:?usage: run-spike.sh <spike directory> <cell>}"

cd "$(dirname "$0")/.."
out="out/$cell"
mkdir -p "$out"

if [ -f "$spike/Cargo.toml" ]; then
  kind=rust
  package="$(sed -n 's/^name *= *"\(.*\)"/\1/p' "$spike/Cargo.toml" | head -n 1)"
elif [ -f "$spike/CMakeLists.txt" ]; then
  kind=c
  package="$(sed -n 's/^project( *\([A-Za-z0-9_-]*\).*/\1/p' "$spike/CMakeLists.txt" | head -n 1)"
else
  echo "$spike has neither Cargo.toml nor CMakeLists.txt" >&2
  exit 1
fi
if [ -z "$package" ]; then
  echo "no package or project name found in $spike" >&2
  exit 1
fi

if [ "$kind" = rust ]; then
  if ! command -v rustup >/dev/null 2>&1; then
    echo "rustup not found: installing it without a default toolchain" >&2
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
      | sh -s -- -y --profile minimal --default-toolchain none --no-modify-path
    export PATH="$HOME/.cargo/bin:$PATH"
  fi
  # Installs the toolchain pinned in rust-toolchain.toml when it is missing.
  rustup toolchain install --profile minimal >/dev/null
fi

{
  echo "cell=$cell"
  echo "spike=$spike"
  echo "kind=$kind"
  echo "package=$package"
  echo "date_utc=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "uname=$(uname -a)"
  echo "runner_os=${RUNNER_OS:-}"
  echo "runner_arch=${RUNNER_ARCH:-}"
  echo "image_os=${ImageOS:-}"
  echo "image_version=${ImageVersion:-}"
  echo "rustflags=${RUSTFLAGS:-}"
  case "$(uname -s)" in
    Linux*)
      echo "os_release=$(. /etc/os-release && echo "$PRETTY_NAME")"
      echo "glibc=$(ldd --version 2>/dev/null | head -n 1)"
      lscpu 2>/dev/null | grep -E '^(Architecture|Vendor ID|Model name|CPU\(s\)|Flags)' || true
      ;;
    Darwin*)
      echo "os_release=$(sw_vers -productName) $(sw_vers -productVersion) ($(sw_vers -buildVersion))"
      echo "cpu=$(sysctl -n machdep.cpu.brand_string)"
      echo "cpu_count=$(sysctl -n hw.ncpu)"
      ;;
    MINGW*|MSYS*|CYGWIN*)
      powershell.exe -NoProfile -Command \
        '"os_release=" + (Get-CimInstance Win32_OperatingSystem).Caption + " " + (Get-CimInstance Win32_OperatingSystem).Version; "cpu=" + (Get-CimInstance Win32_Processor).Name' \
        | tr -d '\r' || true
      ;;
  esac
  if [ "$kind" = rust ]; then
    echo "--- rustc -vV"
    rustc -vV
    echo "--- cargo -vV"
    cargo -vV
  else
    echo "--- cmake --version"
    cmake --version
  fi
} > "$out/environment.txt"

if [ "$kind" = rust ]; then
  cargo build --release --locked --package "$package"
  cargo run --quiet --release --locked --package "$package" -- "$cell" > "$out/facts.json"
else
  build="build/$spike"
  # The configure output names the compiler and the options the spike's
  # dependencies selected; it is kept with the measurements.
  cmake -S "$spike" -B "$build" -DCMAKE_BUILD_TYPE=Release 2>&1 | tee "$out/configure.txt"
  cmake --build "$build" --config Release --parallel
  cmake --install "$build" --config Release --prefix "$build/install" >/dev/null
  {
    echo "--- compiler"
    grep -h -E '^set\(CMAKE_C_COMPILER(_ID|_VERSION)? ' "$build"/CMakeFiles/*/CMakeCCompiler.cmake || true
  } >> "$out/environment.txt"
  exe="$build/install/bin/$package"
  if [ ! -x "$exe" ] && [ -x "$exe.exe" ]; then
    exe="$exe.exe"
  fi
  "$exe" "$cell" > "$out/facts.json"
fi

echo "wrote $out/environment.txt and $out/facts.json"
