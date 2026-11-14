#!/usr/bin/env bash
# Build the pinned QEMU for the Linux matrix and install it under a prefix.
#
# Why not the distribution's package: Ubuntu 24.04 ships QEMU 8.2.2, which
# does not implement the one-byte `int1` instruction (opcode 0xF1; compiled
# out behind WANT_ICEBP until QEMU 9.1.0) and raises #UD for it. The leg
# `fault-contain-bios` asserts the #DB the instruction raises on hardware and
# on the QEMU the project develops against. The pin below is that QEMU, so
# the Linux matrix and the Windows gate run the same emulator.
#
# Usage:  scripts/ci-install-qemu.sh [prefix]     (default: $HOME/.cache/itisyou-qemu/<version>)
# Then:   export PATH="<prefix>/bin:$PATH"
# On a GitHub runner the script appends <prefix>/bin to $GITHUB_PATH itself.
# The tarball's SHA-256 is pinned below; it was checked once against the
# release signature (key CEACC9E15534EBABB82D3FA03353C9CEF108B584).
#
# Idempotent: if <prefix>/bin/qemu-system-x86_64 already reports the pinned
# version, nothing is built. Needs (Ubuntu names): build-essential ninja-build
# pkg-config python3-venv flex bison libglib2.0-dev libpixman-1-dev
# libslirp-dev xz-utils curl. Only the x86_64 system emulator is built.
set -euo pipefail

QEMU_VERSION=11.1.0
QEMU_SHA256=6ee1d1a61f68212476b27108c26da5f449dc09b626d42f8279ba0dc2e08fa858
QEMU_URL="https://download.qemu.org/qemu-${QEMU_VERSION}.tar.xz"

prefix="${1:-$HOME/.cache/itisyou-qemu/${QEMU_VERSION}}"
bin="$prefix/bin/qemu-system-x86_64"

# On a GitHub runner, put the pinned QEMU first on PATH for the later steps.
publish_path() {
    if [ -n "${GITHUB_PATH:-}" ]; then
        echo "$prefix/bin" >> "$GITHUB_PATH"
    fi
}

if [ -x "$bin" ] && "$bin" --version | head -1 | grep -q "version ${QEMU_VERSION}\b"; then
    echo "ci-install-qemu: $("$bin" --version | head -1) already at $prefix"
    publish_path
    exit 0
fi

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
tarball="$work/qemu-${QEMU_VERSION}.tar.xz"

echo "ci-install-qemu: downloading $QEMU_URL"
curl -fsSL --retry 4 --retry-delay 5 -o "$tarball" "$QEMU_URL"
echo "${QEMU_SHA256}  ${tarball}" | sha256sum -c -

tar -C "$work" -xf "$tarball"
mkdir "$work/build"
cd "$work/build"

# slirp is required: the network legs use QEMU's user-mode network. No GUI
# toolkit is needed (the harness runs -display none and reads the screen
# through the monitor).
"../qemu-${QEMU_VERSION}/configure" \
    --prefix="$prefix" \
    --target-list=x86_64-softmmu \
    --enable-slirp \
    --disable-docs --disable-tools --disable-guest-agent \
    --disable-gtk --disable-sdl --disable-opengl --disable-curses \
    --disable-werror
ninja -j"$(nproc)"
ninja install

"$bin" --version | head -1 | grep -q "version ${QEMU_VERSION}\b"
echo "ci-install-qemu: installed $("$bin" --version | head -1) at $prefix"
publish_path
