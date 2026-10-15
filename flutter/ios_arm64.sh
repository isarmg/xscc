#!/usr/bin/env bash
export IPHONEOS_DEPLOYMENT_TARGET=26.0
export SDKROOT="$(xcrun --sdk iphoneos --show-sdk-path)"
cargo build --locked --features flutter,hwcodec --release --target aarch64-apple-ios --lib
