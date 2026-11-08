#!/usr/bin/env bash
set -euo pipefail
[[ $# == 3 ]] || { echo 'Usage: validate-gpui-unix.sh TARGET APP_FEATURES CORE_FEATURES' >&2; exit 2; }
target=$1
features=$2
core_features=$3
manifest=experiments/gpui-validation/Cargo.toml
cargo fmt --manifest-path "$manifest" -- --check
# Preserve the codec-only feature boundary previously checked by the retired UI build.
cargo check --locked -p scrap --lib --no-default-features --target "$target"
cargo tree --locked -p scrap --no-default-features --target "$target" -e normal,build --prefix none
cargo clippy --locked --no-deps --manifest-path "$manifest" --no-default-features --features "$features" --target "$target" --all-targets -- -D warnings
cargo test --locked --manifest-path "$manifest" --no-default-features --features "$features" --target "$target"
pointer_tests=$(cargo test --locked --manifest-path "$manifest" --no-default-features --features "$features" --target "$target" input::tests::clicks_position_the_host_pointer_without_duplicating_drag_events 2>&1) || { printf '%s\n' "$pointer_tests"; exit 1; }
printf '%s\n' "$pointer_tests"
grep -Eq '^test result: ok\. 1 passed; 0 failed;' <<< "$pointer_tests" || { echo 'Pointer click regression did not run' >&2; exit 1; }
scroll_tests=$(cargo test --locked --manifest-path "$manifest" --no-default-features --features "$features" --target "$target" input::tests::scroll_position_matches_pointer_coordinates_and_keeps_delta 2>&1) || { printf '%s\n' "$scroll_tests"; exit 1; }
printf '%s\n' "$scroll_tests"
grep -Eq '^test result: ok\. 1 passed; 0 failed;' <<< "$scroll_tests" || { echo 'Scroll position regression did not run' >&2; exit 1; }
# Test the same native role feature set that ships. Keep every test enabled.
if [[ $target == x86_64-unknown-linux-gnu ]]; then
    # Identify the last native test if the process exits through a signal rather than a Rust panic.
    RUST_BACKTRACE=1 cargo test --locked --manifest-path Cargo.toml --lib --no-default-features --features "$core_features" --target "$target" -- --test-threads=1 --nocapture
    # Keep the original parallel coverage and Cargo's dynamic-library environment.
    # Trace only the test child; retain ASLR and the runner's existing ptrace policy.
    command -v gdb >/dev/null
    xscc_gdb_commands=$(mktemp /tmp/xscc-root-tests.XXXXXX)
    trap 'rm -f -- "$xscc_gdb_commands"' EXIT
    cat > "$xscc_gdb_commands" <<'GDB'
set disable-randomization off
set print thread-events off
set follow-fork-mode parent
set detach-on-fork on
run
thread apply all bt
quit
GDB
    timeout --preserve-status --kill-after=10s 10m env RUST_BACKTRACE=1 \
        CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUNNER="gdb -nx -batch --return-child-result -x $xscc_gdb_commands --args" \
        cargo test --locked --manifest-path Cargo.toml --lib --no-default-features --features "$core_features" --target "$target" -- --nocapture
else
    cargo test --locked --manifest-path Cargo.toml --lib --no-default-features --features "$core_features" --target "$target"
fi
modifier_tests=$(cargo test --locked --manifest-path Cargo.toml --lib --no-default-features --features "$core_features" --target "$target" server::input_service::modifier_keys::tests:: 2>&1) || { printf '%s\n' "$modifier_tests"; exit 1; }
printf '%s\n' "$modifier_tests"
grep -Eq '^test result: ok\. 3 passed; 0 failed;' <<< "$modifier_tests" || { echo 'Modifier ownership regressions did not run all three tests' >&2; exit 1; }
decode_tests=$(cargo test --locked -p hwcodec --lib --target "$target" ffmpeg_ram::decode::decode_output_tests -- --nocapture 2>&1) || { printf '%s\n' "$decode_tests"; exit 1; }
printf '%s\n' "$decode_tests"
grep -Eq '^test result: ok\. 3 passed; 0 failed;' <<< "$decode_tests" || { echo 'Native decoded-plane tests did not run all three tests' >&2; exit 1; }
if [[ $target == aarch64-apple-darwin ]]; then
    audio_tests=$(cargo test --locked --manifest-path Cargo.toml --lib --no-default-features --features "$core_features" --target "$target" client::audio_state_tests::playback_sessions_shrink_backlogs_independently 2>&1) || { printf '%s\n' "$audio_tests"; exit 1; }
    printf '%s\n' "$audio_tests"
    grep -Eq '^test result: ok\. 1 passed; 0 failed;' <<< "$audio_tests" || { echo 'Audio playback backlog regression did not run' >&2; exit 1; }
    quartz_tests=$(cargo test --locked -p scrap --lib --no-default-features --features capture --target "$target" quartz::frame::tests:: -- --nocapture 2>&1) || { printf '%s\n' "$quartz_tests"; exit 1; }
    printf '%s\n' "$quartz_tests"
    grep -Eq '^test result: ok\. 2 passed; 0 failed;' <<< "$quartz_tests" || { echo 'Quartz borrowed-plane tests did not run both tests' >&2; exit 1; }
fi
parse_tests=$(cargo test --locked -p enigo --lib --target "$target" dsl::tests::parse_error_display_describes_the_invalid_input 2>&1) || { printf '%s\n' "$parse_tests"; exit 1; }
printf '%s\n' "$parse_tests"
grep -Eq '^test result: ok\. 1 passed; 0 failed;' <<< "$parse_tests" || { echo 'Enigo parse error regression did not run' >&2; exit 1; }
cargo test --locked -p xscc-native-management --target "$target"
cargo test --locked -p hbb_common --lib --target "$target" tcp::tests
cargo test --locked -p scrap --lib --target "$target" codec::rate_control::tests::
cargo test --locked -p scrap --lib --target "$target" codec::streaming_tests::
limit_tests=$(cargo test --locked -p scrap --lib --target "$target" codec::bitrate_limit_tests:: 2>&1) || { printf '%s\n' "$limit_tests"; exit 1; }
printf '%s\n' "$limit_tests"
grep -Eq '^test result: ok\. [1-9][0-9]* passed;' <<< "$limit_tests" || { echo 'No passing explicit bitrate tests' >&2; exit 1; }
if [[ $target == x86_64-unknown-linux-gnu ]]; then
    compression_tests=$(cargo test --locked -p hbb_common --lib --target "$target" compress::tests:: 2>&1) || { printf '%s\n' "$compression_tests"; exit 1; }
    printf '%s\n' "$compression_tests"
    grep -Eq '^test result: ok\. 3 passed; 0 failed;' <<< "$compression_tests" || { echo 'Compression regressions did not pass all three tests' >&2; exit 1; }
    conversion_tests=$(cargo test --locked -p scrap --lib --target "$target" common::convert::tests:: 2>&1) || { printf '%s\n' "$conversion_tests"; exit 1; }
    printf '%s\n' "$conversion_tests"
    grep -Eq '^test result: ok\. 1 passed; 0 failed;' <<< "$conversion_tests" || { echo 'RGBA conversion regression did not run' >&2; exit 1; }
    # Select only the excluded dependency package while reusing the root lockfile.
    cargo test --locked -p rust-pulsectl --lib --target "$target"
    for filter in upload_staging_reuses_allocation_after_snapshot swizzle_upload_data; do
        output=$(cargo test --locked --manifest-path vendor/gpui/Cargo.toml -p gpui_wgpu --lib --target "$target" "$filter" 2>&1) || { printf '%s\n' "$output"; exit 1; }
        printf '%s\n' "$output"
        grep -Eq '^test result: ok\. [1-9][0-9]* passed;' <<< "$output" || { echo "No passing upload tests: $filter" >&2; exit 1; }
    done
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
