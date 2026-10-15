#!/bin/bash

#
# Script to build F-Droid release of RustDesk
#
# Copyright (C) 2024, The RustDesk Authors
#               2024, Vasyl Gello <vasek.gello@gmail.com>
#

# The script is invoked by F-Droid builder system step-by-step.
#
# It accepts the following arguments:
#
# - versionName from https://github.com/rustdesk/rustdesk/releases/download/fdroid-version/rustdesk-version.txt
# - versionCode from https://github.com/rustdesk/rustdesk/releases/download/fdroid-version/rustdesk-version.txt
# - Android architecture to build APK for: arm64-v8a
# - The build step to execute:
#
#   + prebuild: patch sources and do other stuff before the build
#   + build: perform actual build of APK file
#

# Start of functions

# Install Flutter of version `VERSION` from Github repository
# into directory `FLUTTER_DIR`

prepare_flutter() {
	VERSION="${1}"
	FLUTTER_DIR="${2}"

	if [ ! -f "${FLUTTER_DIR}/bin/flutter" ]; then
		git clone https://github.com/flutter/flutter "${FLUTTER_DIR}"
	fi

	pushd "${FLUTTER_DIR}"

	git restore .
	git checkout "${VERSION}"

	flutter config --no-analytics

	popd # ${FLUTTER_DIR}
}

# Start of script

set -x

# Parse command-line arguments

VERNAME="${1}"
VERCODE="${2}"
ANDROID_ABI="${3}"
BUILDSTEP="${4}"

if [ -z "${VERNAME}" ] || [ -z "${VERCODE}" ] || [ -z "${ANDROID_ABI}" ] ||
	[ -z "${BUILDSTEP}" ]; then
	echo "ERROR: Command-line arguments are all required to be non-empty!" >&2
	exit 1
fi

# Set various architecture-specific identifiers

case "${ANDROID_ABI}" in
arm64-v8a)
	FLUTTER_TARGET=android-arm64
	NDK_TARGET=aarch64-linux-android
	RUST_TARGET=aarch64-linux-android
	;;
*)
	echo "ERROR: Unsupported Android ABI '${ANDROID_ABI}'!" >&2
	exit 1
	;;
esac

# Check ANDROID_SDK_ROOT and sdkmanager present on PATH

if [ ! -d "${ANDROID_SDK_ROOT}" ] || ! command -v sdkmanager 1>/dev/null; then
	echo "ERROR: Can not find Android SDK!" >&2
	exit 1
fi

# Export necessary variables

export PATH="${PATH}:${HOME}/flutter/bin"

export VCPKG_ROOT="${HOME}/vcpkg"

# Now act depending on build step

# NOTE: F-Droid maintainers require explicit declaration of dependencies
# as root via `Builds.sudo` F-Droid metadata directive:
# https://gitlab.com/fdroid/fdroiddata/-/merge_requests/15343#note_1988918695

case "${BUILDSTEP}" in
prebuild)
	# prebuild: patch sources and do other stuff before the build

	#
	# Extract required versions for NDK, Rust, Flutter from
	# '.github/workflows/flutter-build.yml'
	#

	CARGO_NDK_VERSION="$(yq -r \
		.env.CARGO_NDK_VERSION \
		.github/workflows/flutter-build.yml)"

	# Flutter used to compile main Rustdesk library

	FLUTTER_VERSION="$(yq -r \
		.env.ANDROID_FLUTTER_VERSION \
		.github/workflows/flutter-build.yml)"

	if [ -z "${FLUTTER_VERSION}" ]; then
		FLUTTER_VERSION="$(yq -r \
			.env.FLUTTER_VERSION \
			.github/workflows/flutter-build.yml)"
	fi

	# Flutter used to compile Flutter<->Rust bridge files

	CARGO_EXPAND_VERSION="$(yq -r \
		.env.CARGO_EXPAND_VERSION \
		.github/workflows/bridge.yml)"

	FLUTTER_BRIDGE_VERSION="$(yq -r \
		.env.FLUTTER_VERSION \
		.github/workflows/bridge.yml)"

	FLUTTER_RUST_BRIDGE_VERSION="$(yq -r \
		.env.FLUTTER_RUST_BRIDGE_VERSION \
		.github/workflows/bridge.yml)"

	NDK_VERSION="$(yq -r \
		.env.NDK_VERSION \
		.github/workflows/flutter-build.yml)"

	RUST_VERSION="$(yq -r \
		.env.RUST_VERSION \
		.github/workflows/flutter-build.yml)"

	VCPKG_COMMIT_ID="$(yq -r \
		.env.VCPKG_COMMIT_ID \
		.github/workflows/flutter-build.yml)"

	if [ -z "${CARGO_NDK_VERSION}" ] || [ -z "${FLUTTER_VERSION}" ] ||
		[ -z "${FLUTTER_BRIDGE_VERSION}" ] ||
		[ -z "${FLUTTER_RUST_BRIDGE_VERSION}" ] ||
		[ -z "${NDK_VERSION}" ] || [ -z "${RUST_VERSION}" ] ||
		[ -z "${VCPKG_COMMIT_ID}" ]; then
		echo "ERROR: Can not identify all required versions!" >&2
		exit 1
	fi

	# Map NDK version to revision
	NDK_VERSION="$(curl https://gitlab.com/fdroid/android-sdk-transparency-log/-/raw/master/signed/checksums.json |
		jq -r ".\"https://dl.google.com/android/repository/android-ndk-${NDK_VERSION}-linux.zip\"[0].\"source.properties\"" |
		sed -n -E 's/.*Pkg.Revision = ([0-9.]+).*/\1/p')"

	if [ -z "${NDK_VERSION}" ]; then
		echo "ERROR: Can not map Android NDK codename to revision!" >&2
		exit 1
	fi

	export ANDROID_NDK_HOME="${ANDROID_SDK_ROOT}/ndk/${NDK_VERSION}"
	export ANDROID_NDK_ROOT="${ANDROID_SDK_ROOT}/ndk/${NDK_VERSION}"

	#
	# Install the components
	#

	set -e

	# Install Android NDK

	if [ ! -d "${ANDROID_NDK_ROOT}" ]; then
		sdkmanager --install "ndk;${NDK_VERSION}"
	fi

	# Install Rust

	if [ ! -f "${HOME}/rustup/rustup-init.sh" ]; then
		pushd "${HOME}"

		git clone --depth 1 https://github.com/rust-lang/rustup

		popd # ${HOME}
	fi

	pushd "${HOME}/rustup"
	bash rustup-init.sh -y \
		--target "${RUST_TARGET}" \
		--default-toolchain "${RUST_VERSION}"
	popd

	if ! command -v cargo 1>/dev/null 2>&1; then
		. "${HOME}/.cargo/env"
	fi

	# Install cargo-ndk

	cargo install \
		cargo-ndk \
		--version "${CARGO_NDK_VERSION}" \
		--locked

	# Install rust bridge generator

	cargo install \
		cargo-expand \
		--version "${CARGO_EXPAND_VERSION}" \
		--locked
	cargo install flutter_rust_bridge_codegen \
		--version "${FLUTTER_RUST_BRIDGE_VERSION}" \
		--features "uuid" \
		--locked

	# Populate native vcpkg dependencies

	if [ ! -d "${VCPKG_ROOT}" ]; then
		pushd "${HOME}"

		git clone \
			https://github.com/Microsoft/vcpkg.git
		git clone \
			https://github.com/Microsoft/vcpkg-tool.git

		pushd vcpkg-tool

		mkdir build

		pushd build

		cmake \
			-DCMAKE_BUILD_TYPE=Release \
			-G 'Ninja' \
			-DVCPKG_DEVELOPMENT_WARNINGS=OFF \
			..

		cmake --build .

		popd # build

		popd # vcpkg-tool

		pushd vcpkg

		git reset --hard "${VCPKG_COMMIT_ID}"

		cp -a ../vcpkg-tool/build/vcpkg vcpkg

		# disable telemetry

		touch "vcpkg.disable-metrics"

		popd # vcpkg

		popd # ${HOME}
	fi

	# If Flutter version used to generate bridge files differs from Flutter
	# version used to compile Rustdesk library, generate bridge using the
	# `FLUTTER_BRIDGE_VERSION` an restore the pubspec later

	if [ "${FLUTTER_VERSION}" != "${FLUTTER_BRIDGE_VERSION}" ]; then
		# Find first libclang.so and set BRIDGE_LLVM_PATH

		BRIDGE_LLVM_PATH="$(find /usr/lib/ -name libclang.so | head -n1)"

		if [ -z "${BRIDGE_LLVM_PATH}" ]; then
			echo 'ERROR: Can not find libclang.so for bridge generator!' >&2
			exit 1
		fi

		BRIDGE_LLVM_PATH="$(dirname "${BRIDGE_LLVM_PATH}")"
		BRIDGE_LLVM_PATH="$(dirname "${BRIDGE_LLVM_PATH}")"

		# Install Flutter bridge version

		prepare_flutter "${FLUTTER_BRIDGE_VERSION}" "${HOME}/flutter"

		# Save changes

		git add .

		# Download Flutter dependencies

		pushd flutter

		flutter clean
		flutter packages pub get

		popd # flutter

		# Generate FFI bindings

		flutter_rust_bridge_codegen \
			--rust-input ./src/flutter_ffi.rs \
			--dart-output ./flutter/lib/generated_bridge.dart \
			--llvm-path "${BRIDGE_LLVM_PATH}"

		# Add bridge files to save-list

		git add -f ./flutter/lib/generated_bridge.* ./src/bridge_generated.*

		# Restore everything

		git checkout '*'
		git clean -dffx
		git reset

		unset BRIDGE_LLVM_PATH
	fi

	# Install Flutter version for RustDesk library build

	prepare_flutter "${FLUTTER_VERSION}" "${HOME}/flutter"

	# gms is not in these files now, but we still keep the following line for future reference(maybe).

	sed \
		-i \
		-e '/gms/d' \
		flutter/android/build.gradle \
		flutter/android/app/build.gradle

	# `firebase_analytics` is not in these files now, but we still keep the following lines.

	sed \
		-i \
		-e '/firebase_analytics/d' \
		flutter/pubspec.yaml

	sed \
		-i \
		-e '/  firebase/,/    version/d' \
		flutter/pubspec.lock

	sed \
		-i \
		-e '/firebase/Id' \
		flutter/lib/main.dart

	;;
build)
	# build: perform actual build of APK file

	set -e

	#
	# Extract required versions for NDK, Rust, Flutter from
	# '.github/workflows/flutter-build.yml'
	#

	# Flutter used to compile main Rustdesk library

	FLUTTER_VERSION="$(yq -r \
		.env.ANDROID_FLUTTER_VERSION \
		.github/workflows/flutter-build.yml)"

	if [ -z "${FLUTTER_VERSION}" ]; then
		FLUTTER_VERSION="$(yq -r \
			.env.FLUTTER_VERSION \
			.github/workflows/flutter-build.yml)"
	fi

	NDK_VERSION="$(yq -r \
		.env.NDK_VERSION \
		.github/workflows/flutter-build.yml)"

	# Map NDK version to revision
	NDK_VERSION="$(curl https://gitlab.com/fdroid/android-sdk-transparency-log/-/raw/master/signed/checksums.json |
		jq -r ".\"https://dl.google.com/android/repository/android-ndk-${NDK_VERSION}-linux.zip\"[0].\"source.properties\"" |
		sed -n -E 's/.*Pkg.Revision = ([0-9.]+).*/\1/p')"

	if [ -z "${NDK_VERSION}" ]; then
		echo "ERROR: Can not map Android NDK codename to revision!" >&2
		exit 1
	fi

	export ANDROID_NDK_HOME="${ANDROID_SDK_ROOT}/ndk/${NDK_VERSION}"
	export ANDROID_NDK_ROOT="${ANDROID_SDK_ROOT}/ndk/${NDK_VERSION}"

	if ! command -v cargo 1>/dev/null 2>&1; then
		. "${HOME}/.cargo/env"
	fi

	# Download Flutter dependencies

	pushd flutter

	flutter clean
	flutter packages pub get

	popd # flutter

	# Build host android deps

	bash flutter/build_android_deps.sh "${ANDROID_ABI}"

	# Build rustdesk lib

	bash flutter/ndk_arm64.sh

	mkdir -p "flutter/android/app/src/main/jniLibs/${ANDROID_ABI}"

	cp "target/${RUST_TARGET}/release/liblibrustdesk.so" \
		"flutter/android/app/src/main/jniLibs/${ANDROID_ABI}/librustdesk.so"

	cp "${ANDROID_NDK_HOME}/toolchains/llvm/prebuilt/linux-x86_64/sysroot/usr/lib/${NDK_TARGET}/libc++_shared.so" \
		"flutter/android/app/src/main/jniLibs/${ANDROID_ABI}/"

	"${ANDROID_NDK_HOME}/toolchains/llvm/prebuilt/linux-x86_64/bin/llvm-strip" \
		"flutter/android/app/src/main/jniLibs/${ANDROID_ABI}"/*

	# Build the apk

	pushd flutter

	flutter build apk \
		--release \
		--build-number="${VERCODE}" \
		--build-name="${VERNAME}" \
		--target-platform "${FLUTTER_TARGET}"

	popd # flutter

	# Special step for fdroiddata CI builds to remove .gitconfig

	rm -f /home/vagrant/.gitconfig

	;;
*)
	echo "ERROR: Unknown build step '${BUILDSTEP}'!" >&2
	exit 1
	;;
esac

# Report success

echo "All done!"
