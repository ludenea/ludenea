#!/usr/bin/env bash
# What the device baseline spike needs installed on a machine of the research
# CI. On Linux: the Vulkan loader, Mesa's Vulkan drivers (the software
# device), the GLSL compiler that produces the SPIR-V at run time and the
# Vulkan validation layer. On Windows: the optional component that holds
# Direct3D 12's validation layer, asked of the system only on a machine of the
# CI, since it changes the system and needs an administrator. macOS needs
# nothing: Metal and its validation layer ship with the system. The research
# CI runs this script before the build; it prints what it did.
set -euo pipefail

case "$(uname -s)" in
  Linux*)
    if ! command -v apt-get >/dev/null 2>&1; then
      echo "setup: no apt-get on this Linux; the Vulkan path records what it finds"
      exit 0
    fi
    sudo=""
    if [ "$(id -u)" -ne 0 ]; then
      sudo="sudo"
    fi
    export DEBIAN_FRONTEND=noninteractive
    $sudo apt-get update -qq
    $sudo apt-get install -y -qq --no-install-recommends libvulkan1 mesa-vulkan-drivers glslang-tools vulkan-validationlayers
    echo "setup: installed libvulkan1, mesa-vulkan-drivers, glslang-tools and vulkan-validationlayers"
    dpkg-query -W -f='${Package} ${Version}\n' libvulkan1 mesa-vulkan-drivers glslang-tools vulkan-validationlayers 2>/dev/null || true
    ls /usr/share/vulkan/icd.d/ /usr/share/vulkan/explicit_layer.d/ 2>/dev/null || true
    ;;
  MINGW*|MSYS*|CYGWIN*)
    if [ -f "/c/Windows/System32/d3d12SDKLayers.dll" ]; then
      echo "setup: Direct3D 12's validation layer is present (d3d12SDKLayers.dll)"
    elif [ -n "${CI:-}" ]; then
      echo "setup: asking Windows for the Graphics Tools optional component, which holds Direct3D 12's validation layer"
      powershell.exe -NoProfile -Command \
        'try { Add-WindowsCapability -Online -Name "Tools.Graphics.DirectX~~~~0.0.1.0" -ErrorAction Stop | Out-String } catch { "setup: the component could not be added: " + $_.Exception.Message }' \
        | tr -d '\r' || true
      if [ -f "/c/Windows/System32/d3d12SDKLayers.dll" ]; then
        echo "setup: d3d12SDKLayers.dll is present after the request"
      else
        echo "setup: d3d12SDKLayers.dll is absent after the request"
      fi
    else
      echo "setup: Direct3D 12's validation layer is absent (d3d12SDKLayers.dll); outside the CI the spike does not change the system, an administrator adds the Graphics Tools optional component"
    fi
    ;;
  *)
    echo "setup: nothing to install on $(uname -s)"
    ;;
esac
