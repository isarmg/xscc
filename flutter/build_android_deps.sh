#!/bin/bash

set -e -o pipefail

ANDROID_ABI=$1

# Build RustDesk dependencies for Android using vcpkg.json
# Required:
#   1. set VCPKG_ROOT / ANDROID_NDK_HOME path environment variables
#   2. vcpkg initialized
#   3. Android NDK r30 (30.0.16248370)

if [ -z "${ANDROID_NDK_HOME}" ]; then
	echo "ERROR: Please set ANDROID_NDK_HOME environment variable" 1>&2
	exit 1
fi

if [ -z "${VCPKG_ROOT}" ]; then
	echo "ERROR: Please set VCPKG_ROOT environment variable" 1>&2
	exit 1
fi

case "${ANDROID_ABI}" in
arm64-v8a)
	VCPKG_TARGET=arm64-android
	;;
*)
	echo "Usage: build_android_deps.sh <arm64-v8a>" 1>&2
	exit 1
	;;
esac

# Get directory of this script

SCRIPTDIR="$(readlink -f "$0")"
SCRIPTDIR="$(dirname "${SCRIPTDIR}")"

# Check if vcpkg.json is one level up - in root directory of RD

if [ ! -f "${SCRIPTDIR}/../vcpkg.json" ]; then
	echo "ERROR: Can not find vcpkg.json in RustDesk top-level directory" 1>&2
	exit 1
fi

echo "INFO: Building and install vcpkg dependencies for Android ${ANDROID_ABI} ..."

pushd "${SCRIPTDIR}/.."

"${VCPKG_ROOT}/vcpkg" install \
	--triplet "${VCPKG_TARGET}" \
	--x-install-root="${VCPKG_ROOT}/installed"

popd

echo "INFO: Completed building vcpkg dependencies for Android ${ANDROID_ABI}"
