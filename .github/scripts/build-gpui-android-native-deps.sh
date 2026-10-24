#!/usr/bin/env bash
set -euo pipefail

: "${ANDROID_NDK_HOME:?Set ANDROID_NDK_HOME to the Android NDK directory}"
: "${VCPKG_ROOT:?Set VCPKG_ROOT to the initialized vcpkg directory}"
case "${1:-}" in
  arm64-v8a) triplet=arm64-android ;;
  *) echo 'Usage: build-gpui-android-native-deps.sh arm64-v8a' >&2; exit 1 ;;
esac
workspace_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)
test -f "$workspace_root/vcpkg.json"
cd "$workspace_root"
"$VCPKG_ROOT/vcpkg" install --triplet "$triplet" --x-install-root="$VCPKG_ROOT/installed"
