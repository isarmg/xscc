#!/usr/bin/env bash
set -euo pipefail
[[ $# == 3 ]] || { echo 'Usage: validate-gpui-unix.sh TARGET APP_FEATURES CORE_FEATURES' >&2; exit 2; }
target=$1
features=$2
core_features=$3
manifest=experiments/gpui-validation/Cargo.toml
cargo fmt --manifest-path "$manifest" -- --check
cargo clippy --locked --manifest-path "$manifest" --no-default-features --features "$features" --target "$target" --all-targets -- -D warnings
cargo test --locked --manifest-path "$manifest" --no-default-features --features "$features" --target "$target"
# Root defaults enable Flutter; always test the same native feature set that ships.
cargo test --locked --manifest-path Cargo.toml --lib --no-default-features --features "$core_features" --target "$target"
cargo test --locked -p xscc-native-management --target "$target"
if [[ $target == x86_64-unknown-linux-gnu ]]; then
    # Select only the excluded dependency package while reusing the root lockfile.
    cargo test --locked -p rust-pulsectl --lib --target "$target"
fi
# Exercise both modes on each native role without doubling release builds.
cargo test --locked --manifest-path "$manifest" --no-default-features --features "$features,test-lan-direct" --target "$target"
lan_tests=(cargo test --locked --manifest-path Cargo.toml --lib --no-default-features --features "$core_features,test-lan-direct" --target "$target" test_lan::)
listing=$("${lan_tests[@]}" -- --list)
grep -Eq '^test_lan::[^ ]+: test$' <<< "$listing" || { echo 'No test-only LAN regression tests matched' >&2; exit 1; }
"${lan_tests[@]}"
case "${XSCC_PACKAGE_MODE:-managed}" in
    managed) ;;
    test-lan-direct) features+=,test-lan-direct ;;
    *) echo 'Invalid package mode' >&2; exit 2 ;;
esac
cargo build --locked --release --manifest-path "$manifest" --no-default-features --features "$features" --bin xscc-gpui-validation --target "$target"
python3 .github/scripts/test-gpui-unix-cli.py "${CARGO_TARGET_DIR:?}/$target/release/xscc-gpui-validation"
