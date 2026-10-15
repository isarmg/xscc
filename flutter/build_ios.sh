#!/usr/bin/env bash
# https://docs.flutter.dev/deployment/ios
# flutter build ipa --release --obfuscate --split-debug-info=./split-debug-info
# no obfuscate, because no easy to check errors
export IPHONEOS_DEPLOYMENT_TARGET=26.0
flutter build ipa --release
