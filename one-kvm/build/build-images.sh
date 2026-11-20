#!/bin/bash
# Build cross-compiled binaries using cross with custom Dockerfiles
# Usage: ./build/build-images.sh [arch]
# Example: ./build/build-images.sh x86_64

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"

# Cross mounts the current directory. The parent also holds One-KVM's shared
# path dependencies and Cargo registry mirror.
cd "$PROJECT_DIR/.."

# Supported architectures (Rust target)
ARCH_MAP=(
    "x86_64-unknown-linux-gnu"
    "aarch64-unknown-linux-gnu"
    "armv7-unknown-linux-gnueabihf"
)

# Build for specific architecture using cross
build_arch() {
    local rust_target="$1"

    case "${CHINAMIRRO:-}" in
        1|true|TRUE|yes|YES|on|ON)
            # Default Rust toolchain mirror is USTC (rsproxy.cn is unreachable via the
            # Docker bridge on some networks because its CDN blocks the SNAT source IP).
            local rust_dist="${RUSTUP_DIST_SERVER_MIRROR:-https://mirrors.ustc.edu.cn/rust-static}"
            local rust_root="${RUSTUP_UPDATE_ROOT_MIRROR:-https://mirrors.ustc.edu.cn/rust-static/rustup}"
            local cross_build_opts="${CROSS_BUILD_OPTS:+$CROSS_BUILD_OPTS }--progress=plain --build-arg CHINAMIRRO=1 --build-arg DEBIAN_IMAGE=${DEBIAN_IMAGE:-dockerproxy.net/library/debian:12}"
            cross_build_opts="$cross_build_opts --build-arg RUSTUP_DIST_SERVER_CN=$rust_dist --build-arg RUSTUP_UPDATE_ROOT_CN=$rust_root"
            cross_build_opts="$cross_build_opts --build-arg HTTP_PROXY= --build-arg HTTPS_PROXY= --build-arg ALL_PROXY= --build-arg NO_PROXY="
            cross_build_opts="$cross_build_opts --build-arg http_proxy= --build-arg https_proxy= --build-arg all_proxy= --build-arg no_proxy="
            echo "=== China mirror acceleration: enabled ==="
            echo "=== Building: $rust_target (via cross with custom Dockerfile) ==="
            CROSS_BUILD_OPTS="$cross_build_opts" RUSTUP_DIST_SERVER="$rust_dist" RUSTUP_UPDATE_ROOT="$rust_root" \
                cross build --manifest-path one-kvm/Cargo.toml --locked --offline --release --target "$rust_target"
            return
            ;;
    esac

    echo "=== Building: $rust_target (via cross with custom Dockerfile) ==="
    cross build --manifest-path one-kvm/Cargo.toml --locked --offline --release --target "$rust_target"
}

# Main
case "${1:-all}" in
    test)
        cross test --manifest-path one-kvm/Cargo.toml --locked --offline --lib -p one-kvm -p pixel-conversion -p video-codec-backends --target x86_64-unknown-linux-gnu
        exit
        ;;
    all)
        for target in "${ARCH_MAP[@]}"; do
            build_arch "$target"
        done
        ;;
    x86_64|arm64|armv7)
        case "$1" in
            x86_64) build_arch "x86_64-unknown-linux-gnu" ;;
            arm64) build_arch "aarch64-unknown-linux-gnu" ;;
            armv7) build_arch "armv7-unknown-linux-gnueabihf" ;;
        esac
        ;;
    help|--help|-h)
        echo "Usage: $0 [arch|help]"
        echo ""
        echo "Commands:"
        echo "  all     (default) Build all architectures"
        echo "  x86_64  Build only x86_64"
        echo "  arm64   Build only arm64"
        echo "  armv7   Build only ARMv7"
        echo "  test    Test the x86_64 host in its build environment"
        echo ""
        echo "Examples:"
        echo "  $0              # Build all"
        echo "  $0 x86_64       # Build x86_64 only"
        echo "  CHINAMIRRO=1 $0 arm64  # Build with China mirrors"
        exit 0
        ;;
    *)
        echo "Error: Unknown argument: $1"
        exit 1
        ;;
esac

echo ""
echo "Binaries built:"
for target in "${ARCH_MAP[@]}"; do
    if [ -f "$PROJECT_DIR/target/$target/release/one-kvm" ]; then
        echo "  $target: OK"
    fi
done
echo ""
echo "Next step: ./build/package-deb.sh (native packages only)"
