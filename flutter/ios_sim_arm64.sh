#!/usr/bin/env bash
export IPHONEOS_DEPLOYMENT_TARGET=26.0
export SDKROOT="$(xcrun --sdk iphonesimulator --show-sdk-path)"
cargo build --locked --features flutter --release --target aarch64-apple-ios-sim --lib
