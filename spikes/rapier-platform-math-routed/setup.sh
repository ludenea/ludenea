#!/usr/bin/env bash
# What the spike needs before its build, on every machine: the sources of the
# three libraries as published, each verified against the checksum the
# workspace's lock file records, with this spike's patch applied, under
# patched/ (ignored by Git). The spike's manifest replaces the three libraries
# with these copies. On Linux, GDB as well, with which the calls a world's
# steps make to the platform's math library are counted. The research CI runs
# this script before the build; it prints what it did.
set -euo pipefail

cd "$(dirname "$0")"

# Name, version, and the SHA-256 of the archive the registry publishes.
libraries="rapier3d 0.36.1 033e2edc815d61ab97bd74ed5d1cbea5ebe575218774a7cfedbd3551f0033317
parry3d 0.31.1 7b626edc0b497dba4bdf6e1e0797895294ed9f61dac2bd28cb5d34d37ad2ca7e
glamx 0.3.1 28d74079318e3a077bd3d15faa90fde24fa37bb9f157f92f853e6019a15fd413"

# The SHA-256 of standard input.
hash_of() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum | cut -d' ' -f1
  else
    shasum -a 256 | cut -d' ' -f1
  fi
}

needed="curl tar patch"
case "$(uname -s)" in
  Linux*) needed="$needed gdb" ;;
esac
missing=""
for tool in $needed; do
  command -v "$tool" >/dev/null 2>&1 || missing="$missing $tool"
done
if [ -n "$missing" ] && command -v apt-get >/dev/null 2>&1; then
  sudo=""
  if [ "$(id -u)" -ne 0 ]; then
    sudo="sudo"
  fi
  export DEBIAN_FRONTEND=noninteractive
  $sudo apt-get update -qq
  # shellcheck disable=SC2086 # a list of package names
  $sudo apt-get install -y -qq --no-install-recommends ca-certificates $missing
  echo "setup: installed$missing"
fi
for tool in curl tar patch; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "setup: $tool is missing and could not be installed; the patched sources cannot be made" >&2
    exit 1
  fi
done
case "$(uname -s)" in
  Linux*)
    if command -v gdb >/dev/null 2>&1; then
      gdb --version | head -n 1
    else
      echo "setup: no GDB on this Linux; the spike records that the calls are not counted"
    fi
    ;;
esac

# The patched sources are made again whenever the libraries or the patches
# change, and only then, so that a second local build has nothing to rebuild.
stamp="$({
  echo "$libraries"
  for patch_file in patches/*.patch; do
    tr -d '\r' < "$patch_file"
  done
} | hash_of)"
if [ -f patched/.stamp ] && [ "$(cat patched/.stamp)" = "$stamp" ]; then
  echo "setup: the patched sources are in place"
else
  rm -rf patched
  mkdir -p patched
  while read -r name version checksum; do
    archive="$name-$version.crate"
    if ! grep -q "checksum = \"$checksum\"" ../Cargo.lock; then
      echo "setup: $checksum, stated here for $name $version, is not a checksum of the workspace's lock file" >&2
      exit 1
    fi
    (
      cd patched
      curl --proto '=https' --tlsv1.2 -sSfL --retry 3 -o "$archive" "https://static.crates.io/crates/$name/$archive"
      got="$(hash_of < "$archive")"
      if [ "$got" != "$checksum" ]; then
        echo "setup: $archive hashes to $got, not to $checksum" >&2
        exit 1
      fi
      mkdir "$name"
      tar -xzf "$archive" -C "$name" --strip-components=1
      rm "$archive"
    )
    # A patch file checked out with the line endings of Windows still applies.
    tr -d '\r' < "patches/$name-$version.patch" | (cd "patched/$name" && patch -p1 -N -s)
    echo "setup: $name $version as published (SHA-256 verified), with patches/$name-$version.patch applied"
  done <<EOF
$libraries
EOF
  echo "$stamp" > patched/.stamp
fi

# What the build must find: each call in its routed form.
holds() {
  if ! grep -qF -- "$2" "patched/$1"; then
    echo "setup: patched/$1 does not hold: $2" >&2
    exit 1
  fi
}
holds rapier3d/src/dynamics/solver/joint_constraint/joint_constraint_helper.rs 'ComplexField::sin_cos(center * 0.5)'
holds rapier3d/src/dynamics/solver/joint_constraint/generic_joint_constraint_builder.rs 'ComplexField::sin(motor_params.target_pos / 2.0)'
holds parry3d/src/partitioning/bvh/bvh_optimize.rs 'ComplexField::log2(target_root_node_count)'
holds parry3d/src/partitioning/bvh/bvh_optimize.rs 'ComplexField::log2(target_subtree_leaf_count)'
holds parry3d/src/shape/convex_polyhedron.rs 'ComplexField::sin_cos(eps)'
holds glamx/src/eigen3.rs '<$Real as ComplexField>::acos(r)'
holds glamx/src/eigen3.rs '<$Real as ComplexField>::cos(phi)'
holds glamx/src/eigen3.rs '<$Real as ComplexField>::cos(phi + 2.0 * pi / 3.0)'
echo "setup: the five calls are in their routed form in the patched sources"
