#!/usr/bin/env bash
# What the device layer probe needs installed on a Linux machine to build: the
# gamepad library under study links the system's device manager library
# (libudev) and finds it through pkg-config. Windows and macOS need nothing.
# The research CI runs this script before the build; it prints what it did.
set -euo pipefail

case "$(uname -s)" in
  Linux*)
    if ! command -v apt-get >/dev/null 2>&1; then
      echo "setup: no apt-get on this Linux; the build needs libudev and pkg-config already present"
      exit 0
    fi
    sudo=""
    if [ "$(id -u)" -ne 0 ]; then
      sudo="sudo"
    fi
    export DEBIAN_FRONTEND=noninteractive
    $sudo apt-get update -qq
    $sudo apt-get install -y -qq --no-install-recommends libudev-dev pkg-config
    echo "setup: installed libudev-dev and pkg-config"
    dpkg-query -W -f='${Package} ${Version}\n' libudev-dev pkg-config 2>/dev/null || true
    ;;
  *)
    echo "setup: nothing to install on $(uname -s)"
    ;;
esac
