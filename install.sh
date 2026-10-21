#!/bin/sh
# Per-user install into ~/.local (no root needed).
#   ./install.sh               install binaries, desktop entry and icons
#   ./install.sh --uninstall   remove them (settings and tokens are kept)
# Works from a release archive (binaries next to this script) or from a
# source checkout after `cargo build --release`.
# No udev rule is needed: the kit is reached through the Netatmo cloud.
set -eu

PREFIX="${PREFIX:-$HOME/.local}"
BIN="$PREFIX/bin"
APPS="$PREFIX/share/applications"
ICONS="$PREFIX/share/icons/hicolor"
HERE="$(cd "$(dirname "$0")" && pwd)"
NAMES="bticino-easykit bticino-easykit-gui"

find_bin() {
    for d in "$HERE" "$HERE/target/release"; do
        [ -x "$d/$1" ] && { echo "$d/$1"; return; }
    done
    echo "missing $1 (build with: cargo build --release)" >&2
    exit 1
}

asset() {
    for p in "$HERE/$1" "$HERE/$(basename "$1")"; do
        [ -f "$p" ] && { echo "$p"; return; }
    done
    echo "missing $1" >&2
    exit 1
}

if [ "${1:-}" = "--uninstall" ]; then
    for n in $NAMES; do rm -f "$BIN/$n"; done
    rm -f "$APPS/bticino-easykit.desktop"
    for s in 16 32 48 64 128 256 512; do rm -f "$ICONS/${s}x${s}/apps/bticino-easykit.png"; done
    rm -f "$ICONS/scalable/apps/bticino-easykit.svg"
    echo "Removed. Settings remain in ${XDG_CONFIG_HOME:-$HOME/.config}/bticino-easykit"
    exit 0
fi

mkdir -p "$BIN" "$APPS"
for n in $NAMES; do install -m 755 "$(find_bin "$n")" "$BIN/$n"; done
install -m 644 "$(asset packaging/linux/bticino-easykit.desktop)" "$APPS/bticino-easykit.desktop"
for s in 16 32 48 64 128 256 512; do
    install -D -m 644 "$(asset "assets/icons/bticino-easykit-$s.png")" "$ICONS/${s}x${s}/apps/bticino-easykit.png"
done
install -D -m 644 "$(asset assets/icons/bticino-easykit.svg)" "$ICONS/scalable/apps/bticino-easykit.svg"
command -v update-desktop-database >/dev/null && update-desktop-database "$APPS" 2>/dev/null || true
command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q "$ICONS" 2>/dev/null || true
echo "Installed into $PREFIX. Make sure $BIN is in your PATH."
