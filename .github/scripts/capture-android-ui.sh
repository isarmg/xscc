#!/usr/bin/env bash
# Actual application capture only: no fixtures, service enrollment, or rendered substitutes.
set -euo pipefail
APK=$(realpath "${1:?APK path required}")
OUT=$(realpath -m "${2:?Evidence directory required}")
SDK=${ANDROID_HOME:?Android SDK required}
ADB="$SDK/platform-tools/adb"
EMULATOR="$SDK/emulator/emulator"
APP=org.sarmg.xscc.gpui.testdirect
ACTIVITY=org.xscc.gpui.smoke.SmokeActivity
SERIAL=emulator-5554
mkdir -p "$OUT"
emulator_pid=
finish() {
  result=$?
  "$ADB" -s "$SERIAL" logcat -d > "$OUT/logcat.txt" 2>&1 || true
  "$ADB" -s "$SERIAL" shell dumpsys activity activities > "$OUT/activities.txt" 2>&1 || true
  "$ADB" -s "$SERIAL" shell dumpsys window windows > "$OUT/windows.txt" 2>&1 || true
  printf 'script_exit=%s\nvisual_review=required\n' "$result" > "$OUT/RESULT.txt"
  "$ADB" -s "$SERIAL" emu kill >/dev/null 2>&1 || true
  if [[ -n "$emulator_pid" ]]; then
    kill "$emulator_pid" 2>/dev/null || true
    wait "$emulator_pid" || true
  fi
  exit "$result"
}
trap finish EXIT
{
  printf 'source_commit=%s\nsource_run=%s\nsource_artifact=%s\nsource_zip_sha256=%s\n' \
    "$SOURCE_SHA" "$SOURCE_RUN" "$SOURCE_ARTIFACT" "$SOURCE_ZIP_SHA256"
  printf 'capture=actual-Android-emulator-screen\nexecution=ARM64-APK-via-native-bridge-on-x86_64\n'
  printf 'system_image=system-images;android-30;google_apis;x86_64\ngpu=swiftshader\n'
  printf 'service_enrollment=none\nhost_fixtures=none\nphysical_device=false\n'
  sha256sum "$APK"
  uname -a
  "$EMULATOR" -version
} > "$OUT/ENVIRONMENT.txt" 2>&1
printf 'no\n' | "$SDK/cmdline-tools/latest/bin/avdmanager" create avd \
  --force --name xscc-ui-capture --package 'system-images;android-30;google_apis;x86_64' --device pixel_5
"$EMULATOR" -avd xscc-ui-capture -port 5554 -no-window -no-audio \
  -no-boot-anim -no-snapshot -wipe-data -gpu swiftshader -accel on \
  > "$OUT/emulator.txt" 2>&1 &
emulator_pid=$!
booted=false
for _ in $(seq 1 120); do
  kill -0 "$emulator_pid" || { echo 'Emulator exited before boot' >&2; exit 1; }
  if [[ $("$ADB" -s "$SERIAL" shell getprop sys.boot_completed 2>/dev/null | tr -d '\r') == 1 ]]; then
    booted=true
    break
  fi
  sleep 3
done
[[ "$booted" == true ]] || { echo 'Emulator did not finish booting within six minutes' >&2; exit 1; }
"$ADB" -s "$SERIAL" shell getprop > "$OUT/device-properties.txt"
abis=$("$ADB" -s "$SERIAL" shell getprop ro.product.cpu.abilist | tr -d '\r')
bridge=$("$ADB" -s "$SERIAL" shell getprop ro.dalvik.vm.native.bridge | tr -d '\r')
[[ ",$abis," == *,arm64-v8a,* ]] || { echo "System image cannot execute ARM64 APK: $abis" >&2; exit 1; }
[[ -n "$bridge" && "$bridge" != 0 ]] || { echo 'No active ARM native bridge; cannot claim ARM64 execution' >&2; exit 1; }
printf 'reported_abis=%s\nnative_bridge=%s\n' "$abis" "$bridge" >> "$OUT/ENVIRONMENT.txt"
"$ADB" -s "$SERIAL" shell input keyevent KEYCODE_WAKEUP
"$ADB" -s "$SERIAL" shell wm dismiss-keyguard
"$ADB" -s "$SERIAL" install --abi arm64-v8a "$APK" | tee "$OUT/install.txt"
"$ADB" -s "$SERIAL" shell dumpsys package "$APP" > "$OUT/package.txt"
grep -q 'primaryCpuAbi=arm64-v8a' "$OUT/package.txt"
"$ADB" -s "$SERIAL" logcat -c
"$ADB" -s "$SERIAL" shell am start -W -n "$APP/$ACTIVITY" | tee "$OUT/launch.txt"
grep -q 'Status: ok' "$OUT/launch.txt"
elapsed=0
for delay in 5 10 15; do
  sleep "$delay"
  elapsed=$((elapsed + delay))
  pid=$("$ADB" -s "$SERIAL" shell pidof "$APP" | tr -d '\r')
  [[ "$pid" =~ ^[0-9]+$ ]] || { echo 'Application is not running' >&2; exit 1; }
  "$ADB" -s "$SERIAL" shell dumpsys activity activities > "$OUT/activity-${elapsed}s.txt"
  grep -E 'mResumedActivity|topResumedActivity' "$OUT/activity-${elapsed}s.txt" | grep -F "$APP/"
  "$ADB" -s "$SERIAL" logcat -d --pid="$pid" > "$OUT/app-${elapsed}s-logcat.txt"
  if grep -Eiq 'FATAL EXCEPTION|Fatal signal|panicked at|Cannot start the management preview|Could not open the management preview|no suitable adapter' "$OUT/app-${elapsed}s-logcat.txt"; then
    echo 'Native application reported a startup/render failure' >&2
    exit 1
  fi
  "$ADB" -s "$SERIAL" exec-out screencap -p > "$OUT/android-home-${elapsed}s.png"
  python3 - "$OUT/android-home-${elapsed}s.png" <<'PY'
import struct, sys
with open(sys.argv[1], 'rb') as f:
    header = f.read(24)
assert header[:8] == b'\x89PNG\r\n\x1a\n' and header[12:16] == b'IHDR', 'Invalid screenshot PNG'
width, height = struct.unpack('>II', header[16:24])
assert width >= 300 and height >= 300, 'Screenshot is too small'
print(f'Actual emulator screenshot: {width}x{height}; visual inspection still required')
PY
done
printf 'capture_result=application-running-screenshots-captured\n' >> "$OUT/ENVIRONMENT.txt"
