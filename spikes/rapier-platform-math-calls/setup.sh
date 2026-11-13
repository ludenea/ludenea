#!/usr/bin/env bash
# What the spike needs installed on a machine of the research CI. On Linux:
# GDB, with which the spike counts the calls a world's steps make to the
# platform's math library. Elsewhere nothing: the calls are counted on Linux
# alone, and the functions and the worlds need nothing but the build. The
# research CI runs this script before the build; it prints what it did.
set -euo pipefail

case "$(uname -s)" in
  Linux*)
    if command -v gdb >/dev/null 2>&1; then
      echo "setup: GDB is present"
    elif command -v apt-get >/dev/null 2>&1; then
      sudo=""
      if [ "$(id -u)" -ne 0 ]; then
        sudo="sudo"
      fi
      export DEBIAN_FRONTEND=noninteractive
      $sudo apt-get update -qq
      $sudo apt-get install -y -qq --no-install-recommends gdb
      echo "setup: installed GDB"
    else
      echo "setup: no apt-get on this Linux; the spike records that the calls are not counted"
      exit 0
    fi
    gdb --version | head -n 1
    ;;
  *)
    echo "setup: nothing to install on $(uname -s)"
    ;;
esac
