#!/usr/bin/env bash
# What the audio spike needs installed on a Linux machine to build and to
# start: the output library under study links the ALSA library (libasound) and
# finds it through pkg-config, and a binary that links it does not start where
# it is absent. Windows and macOS need nothing. The research CI runs this
# script before the build; it prints what it did.
set -euo pipefail

case "$(uname -s)" in
  Linux*)
    if ! command -v apt-get >/dev/null 2>&1; then
      echo "setup: no apt-get on this Linux; the build needs libasound and pkg-config already present"
      exit 0
    fi
    sudo=""
    if [ "$(id -u)" -ne 0 ]; then
      sudo="sudo"
    fi
    export DEBIAN_FRONTEND=noninteractive
    $sudo apt-get update -qq
    $sudo apt-get install -y -qq --no-install-recommends libasound2-dev pkg-config
    echo "setup: installed libasound2-dev and pkg-config"
    dpkg-query -W -f='${Package} ${Version}\n' libasound2-dev pkg-config 2>/dev/null || true
    echo "setup: sound devices under /dev/snd: $(ls /dev/snd 2>/dev/null | tr '\n' ' ')"
    echo "setup: audio daemons running: $(ps -e -o comm= 2>/dev/null | grep -E 'pipewire|pulseaudio|wireplumber' | sort -u | tr '\n' ' ')"
    ;;
  *)
    echo "setup: nothing to install on $(uname -s)"
    ;;
esac
