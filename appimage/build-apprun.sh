#!/usr/bin/env bash
# AppRun v2.0.0, which appimage-builder downloads, splits every argument into a 1 KiB stack
# buffer, so any argument of 1024 bytes or more aborts the AppImage before RustDesk starts.
# Build it with apprun-split-arguments.patch into the path appimage-builder checks before
# downloading. Build on the same Ubuntu 26.04 baseline as the packaged application.
set -euo pipefail

arch=$1
version=v2.0.0
commit=407700ccef58e1a64cc49856074aee4b1c75d599
sha256=05729f2ae0a744369d2d19d0f37acfbdcb1ee064172955b7576883ccef5113b3
image=ubuntu:26.04
here=$(cd "$(dirname "$0")" && pwd)

case $arch in
  x86_64) packages="cmake make g++" ;;
  *) echo "Unsupported arch: $arch" >&2; exit 1 ;;
esac

tmp=$(mktemp -d)
src=$tmp/src
curl -fsSL -o "$tmp/apprun.tar.gz" "https://github.com/AppImageCrafters/AppRun/archive/$commit.tar.gz"
echo "$sha256  $tmp/apprun.tar.gz" | sha256sum -c -
mkdir "$src"
tar -xzf "$tmp/apprun.tar.gz" -C "$src" --strip-components=1
patch -d "$src" -p1 < "$here/apprun-split-arguments.patch"
# CMake 4 removed policy compatibility below 3.5.
sed -i 's/cmake_minimum_required(VERSION 3.2)/cmake_minimum_required(VERSION 3.5)/' "$src/CMakeLists.txt"

docker run --rm -v "$src:/src" -w /src -e DEBIAN_FRONTEND=noninteractive "$image" bash -c "
  set -e
  apt-get update
  apt-get install -y --no-install-recommends $packages
  mkdir build && cd build
  cmake .. -DCMAKE_BUILD_TYPE=Release -DCMAKE_TOOLCHAIN_FILE=../cmake/$arch-toolchain.cmake
  cmake --build . --target AppRun"

mkdir -p "$here/appimage-build/AppRun/$version"
cp "$src/build/src/apprun/AppRun" "$here/appimage-build/AppRun/$version/AppRun-Release-$arch"
