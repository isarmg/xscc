#!/usr/bin/env bash
set -euo pipefail
if [[ $(uname -s) != Linux || $(uname -m) != x86_64 ]]; then
    echo 'XSCC requires native Linux x86_64.' >&2
    exit 1
fi
if [[ ${XDG_SESSION_TYPE:-} != wayland && -z ${WAYLAND_DISPLAY:-} ]]; then
    echo 'Start XSCC from your logged-in Wayland desktop session.' >&2
    exit 1
fi
if (( EUID == 0 )); then
    echo 'Start this foreground package as your desktop user, without sudo.' >&2
    exit 1
fi
case "${1:-}" in
    --server|--service|--install-service-only|--uninstall-service-only)
        echo 'This package supports a foreground Wayland session only.' >&2
        exit 1
        ;;
esac
here=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
read -r role < "$here/role"
case "$role" in controller|host) ;; *) echo 'Invalid package role' >&2; exit 1 ;; esac
exec "$here/xscc-$role" "$@"
