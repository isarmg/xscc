#!/usr/bin/env bash
set -euo pipefail
[[ $# == 4 ]] || { echo 'Usage: package-gpui-linux.sh controller|host VERSION EXECUTABLE OUTPUT_DIR' >&2; exit 2; }
role=$1
version=$2
executable=$3
output=$4
case "$role" in controller|host) ;; *) echo 'Invalid package role' >&2; exit 2 ;; esac
[[ $version =~ ^[0-9]+\.[0-9]+\.[0-9]+-beta\.[0-9]+$ ]] || { echo 'Expected version major.minor.patch-beta.revision' >&2; exit 2; }
[[ -f $executable && -x $executable ]] || { echo 'Executable is missing or not executable' >&2; exit 1; }
file "$executable" | grep -Eq 'ELF 64-bit LSB.*x86-64' || { echo 'Expected a Linux x86_64 ELF executable' >&2; exit 1; }
root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)
mkdir -p -- "$output"
output=$(cd -- "$output" && pwd)
name="xscc-$role-$version-linux-x86_64"
[[ ! -e $output/$name.tar.gz ]] || { echo 'Refusing to overwrite an existing package' >&2; exit 1; }
staging=$(mktemp -d)
trap 'rm -rf -- "$staging"' EXIT
mkdir -- "$staging/$name"
python3 "$root/.github/scripts/check-gpui-package-mode.py" "$executable" --output "$staging/$name/build-capabilities.json"
install -m 755 -- "$executable" "$staging/$name/xscc-$role"
install -m 755 -- "$root/packaging/gpui-linux/launch.sh" "$staging/$name/launch.sh"
printf '%s\n' "$role" > "$staging/$name/role"
cp -- "$root/packaging/gpui-linux/README.txt" "$staging/$name/README.txt"
cp -- "$root/LICENCE" "$staging/$name/LICENCE"
cp -- "$root/packaging/gpui-windows/GPUI-LICENSE-APACHE.txt" "$staging/$name/GPUI-LICENSE-APACHE.txt"
cp -- "$root/packaging/gpui-windows/THIRD_PARTY_NOTICES.txt" "$staging/$name/THIRD_PARTY_NOTICES.txt"
tar -czf "$output/$name.tar.gz" -C "$staging" "$name"
(cd -- "$output" && sha256sum "$name.tar.gz" > "$name.tar.gz.sha256")
echo "$output/$name.tar.gz"
