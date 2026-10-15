#!/usr/bin/env bash

MODE=${MODE:=release}
"${ANDROID_NDK_HOME}/toolchains/llvm/prebuilt/linux-x86_64/bin/llvm-strip" android/app/src/main/jniLibs/arm64-v8a/*
flutter build apk --target-platform android-arm64 --${MODE} --obfuscate --split-debug-info ./split-debug-info
flutter build appbundle --target-platform  android-arm64 --${MODE} --obfuscate --split-debug-info ./split-debug-info
