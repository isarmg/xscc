#!/usr/bin/env bash
set -eo pipefail

bindgen_args="$(
    cargo ndk-env --platform 29 --target aarch64-linux-android --json |
        python3 -c 'import json, sys; print(json.load(sys.stdin)["BINDGEN_EXTRA_CLANG_ARGS_aarch64_linux_android"])'
)"
# cargo-ndk 4.1.2 overwrites the underscore key without an API target. Bindgen
# prefers this hyphenated key; keep its NDK sysroot and add the API required by r30.
env "BINDGEN_EXTRA_CLANG_ARGS_aarch64-linux-android=$bindgen_args --target=aarch64-linux-android29" \
    cargo ndk --platform 29 --target aarch64-linux-android build --locked --release --features flutter,mediacodec --lib
