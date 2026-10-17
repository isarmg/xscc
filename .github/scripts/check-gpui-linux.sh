#!/usr/bin/env bash
set -euo pipefail
mode=${1:-check}
role=${2:-all}
case "$mode" in preflight|check|test) ;; *) echo 'Usage: check-gpui-linux.sh [preflight|check|test] [controller|host|all]' >&2; exit 2 ;; esac
case "$role" in controller|host|all) ;; *) echo 'Role must be controller, host, or all' >&2; exit 2 ;; esac
[[ $(uname -s) == Linux && $(uname -m) == x86_64 ]] || { echo 'Native Linux x86_64 is required' >&2; exit 1; }
root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)
missing=0
for command in cargo pkg-config clang; do
    if ! command -v "$command" >/dev/null; then echo "Missing build tool: $command" >&2; missing=1; fi
done
if command -v pkg-config >/dev/null; then
    for library in glib-2.0 gtk+-3.0 gstreamer-1.0 gstreamer-app-1.0 gstreamer-video-1.0 libpulse libpulse-simple dbus-1 xkbcommon wayland-client fontconfig openssl; do
        if ! pkg-config --exists "$library"; then echo "Missing pkg-config development package: $library" >&2; missing=1; fi
    done
fi
for header in libavcodec/avcodec.h opus/opus.h vpx/vpx_decoder.h aom/aom_decoder.h libyuv.h; do
    if [[ ! -f ${VCPKG_ROOT:-}/installed/x64-linux/include/$header ]]; then
        echo "Missing pinned vcpkg x64-linux header: $header (set VCPKG_ROOT)" >&2
        missing=1
    fi
done
(( missing == 0 )) || exit 1
[[ $mode != preflight ]] || { echo 'Linux native build prerequisites found'; exit 0; }
roles=("$role")
[[ $role != all ]] || roles=(controller host)
for selected in "${roles[@]}"; do
    feature=live-session
    [[ $selected != host ]] || feature=native-host
    echo "Validating Linux $selected ($mode)"
    cargo "$mode" --locked --manifest-path "$root/experiments/gpui-validation/Cargo.toml" --features "$feature"
done
