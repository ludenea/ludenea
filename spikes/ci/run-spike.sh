#!/usr/bin/env bash
# Builds one spike with the pinned toolchain, runs it, and writes its
# measurements and the facts about the machine under out/<cell>/.
#
# Usage: ci/run-spike.sh <spike directory> <cell>
#   <spike directory>  a member of the spikes workspace, for example platform-facts
#   <cell>             a label for the machine, for example linux-x86_64
#
# Portable on purpose, so that the research CI never depends on GitHub Actions
# features: it runs unchanged on Linux, macOS and Windows (Git Bash), hosted or local.
set -euo pipefail

spike="${1:?usage: run-spike.sh <spike directory> <cell>}"
cell="${2:?usage: run-spike.sh <spike directory> <cell>}"

cd "$(dirname "$0")/.."
out="out/$cell"
mkdir -p "$out"

if ! command -v rustup >/dev/null 2>&1; then
  echo "rustup not found: installing it without a default toolchain" >&2
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
    | sh -s -- -y --profile minimal --default-toolchain none --no-modify-path
  export PATH="$HOME/.cargo/bin:$PATH"
fi
# Installs the toolchain pinned in rust-toolchain.toml when it is missing.
rustup toolchain install --profile minimal >/dev/null

package="$(sed -n 's/^name *= *"\(.*\)"/\1/p' "$spike/Cargo.toml" | head -n 1)"
if [ -z "$package" ]; then
  echo "no package name found in $spike/Cargo.toml" >&2
  exit 1
fi

{
  echo "cell=$cell"
  echo "spike=$spike"
  echo "package=$package"
  echo "date_utc=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "uname=$(uname -a)"
  echo "runner_os=${RUNNER_OS:-}"
  echo "runner_arch=${RUNNER_ARCH:-}"
  echo "image_os=${ImageOS:-}"
  echo "image_version=${ImageVersion:-}"
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
  echo "--- rustc -vV"
  rustc -vV
  echo "--- cargo -vV"
  cargo -vV
} > "$out/environment.txt"

cargo build --release --locked --package "$package"
cargo run --quiet --release --locked --package "$package" -- "$cell" > "$out/facts.json"

echo "wrote $out/environment.txt and $out/facts.json"
