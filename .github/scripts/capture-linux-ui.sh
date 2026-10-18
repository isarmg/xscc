#!/usr/bin/env bash
# Capture the packaged application in a real Wayland session on a virtual display.
set -euo pipefail
[[ $# == 2 ]] || { echo 'Usage: capture-linux-ui.sh EXECUTABLE OUTPUT_DIR' >&2; exit 2; }
(( EUID != 0 )) || { echo 'Run the GUI as the disposable runner desktop user, not root.' >&2; exit 1; }
exe=$(realpath "$1")
out=$(realpath -m "$2")
mkdir -p "$out"
if [[ -z ${DBUS_SESSION_BUS_ADDRESS:-} ]]; then
    exec dbus-run-session -- bash "$0" "$exe" "$out"
fi
export XDG_RUNTIME_DIR="$(mktemp -d)"
chmod 700 "$XDG_RUNTIME_DIR"
export XDG_SESSION_TYPE=wayland
export WLR_BACKENDS=headless WLR_HEADLESS_OUTPUTS=1 WLR_LIBINPUT_NO_DEVICES=1 WLR_RENDERER=pixman
export LIBGL_ALWAYS_SOFTWARE=1
icd=$(find /usr/share/vulkan/icd.d -name '*lvp*.json' -print -quit)
[[ -n $icd ]] || { echo 'Mesa Lavapipe Vulkan driver is missing.' >&2; exit 1; }
export VK_DRIVER_FILES="$icd" VK_ICD_FILENAMES="$icd"
unset DISPLAY
cat > "$out/sway.conf" <<'CONFIG'
xwayland disable
output HEADLESS-1 mode 1440x1000
output * bg #263238 solid_color
font pango:Noto Sans CJK SC 12
default_border normal 2
for_window [title="^XSCC"] floating enable
seat seat0 fallback true
CONFIG
app_pid=''
sway_pid=''
cleanup() {
    [[ -z $app_pid ]] || kill "$app_pid" 2>/dev/null || true
    [[ -z $sway_pid ]] || kill "$sway_pid" 2>/dev/null || true
}
trap cleanup EXIT
sway --config "$out/sway.conf" --verbose >"$out/sway.log" 2>&1 &
sway_pid=$!
for _ in $(seq 1 40); do
    kill -0 "$sway_pid" || { cat "$out/sway.log"; exit 1; }
    socket=$(find "$XDG_RUNTIME_DIR" -maxdepth 1 -type s -name 'sway-ipc.*.sock' -print -quit)
    display=$(find "$XDG_RUNTIME_DIR" -maxdepth 1 -type s -name 'wayland-*' -printf '%f\n' -quit)
    if [[ -n $socket && -n $display ]]; then break; fi
    sleep 0.5
done
[[ -n ${socket:-} && -n ${display:-} ]] || { echo 'Wayland compositor did not start.' >&2; exit 1; }
export SWAYSOCK="$socket" WAYLAND_DISPLAY="$display"
swaymsg -t get_outputs > "$out/outputs.json"
(cd "$(dirname "$exe")" && exec "$exe") >"$out/application.log" 2>&1 &
app_pid=$!
for _ in $(seq 1 60); do
    kill -0 "$app_pid" || { cat "$out/application.log"; exit 1; }
    swaymsg -t get_tree > "$out/tree.json"
    window=$(jq -c --argjson pid "$app_pid" '.. | objects | select(.pid? == $pid and (.name? // "" | startswith("XSCC")))' "$out/tree.json" | head -1)
    [[ -z $window ]] || break
    sleep 0.5
done
[[ -n ${window:-} ]] || { echo 'No real XSCC application window mapped; refusing a wallpaper screenshot.' >&2; exit 1; }
id=$(jq -r '.id' <<< "$window")
swaymsg "[con_id=$id] focus" >/dev/null
sleep 3
kill -0 "$app_pid"
swaymsg -t get_tree > "$out/tree.json"
window=$(jq -c --argjson id "$id" '.. | objects | select(.id? == $id)' "$out/tree.json")
jq -e '.visible == true and .rect.width >= 300 and .rect.height >= 200' <<< "$window" >/dev/null
geometry=$(jq -r '.rect | "\(.x),\(.y) \(.width)x\(.height)"' <<< "$window")
grim -g "$geometry" "$out/linux-wayland-window.png"
grim "$out/linux-wayland-desktop.png"
python3 - "$out/linux-wayland-window.png" <<'PY'
from PIL import Image
import sys
im = Image.open(sys.argv[1]).convert('RGB')
# A flat/empty GPU surface is not evidence of a rendered UI.
colors = im.crop((8, 50, im.width - 8, im.height - 8)).resize((160, 120)).getcolors(19200)
if colors is None or len(colors) < 12:
    raise SystemExit('Captured client area is blank or too uniform; inspect logs, do not publish it as a UI screenshot.')
PY
{
    printf 'source=%s\n' "${SOURCE_SHA:?}"
    printf 'platform=Linux Ubuntu 26.04; GitHub Actions; headless Sway Wayland; Lavapipe software Vulkan\n'
    printf 'visual_review=required; color checks only reject obviously blank frames\n'
    printf 'capture=actual packaged controller window; no test peers or remote session injected\n'
    printf 'pid=%s\nwindow_id=%s\ngeometry=%s\n' "$app_pid" "$id" "$geometry"
    sha256sum "$exe" "$out/linux-wayland-window.png"
} > "$out/provenance.txt"
