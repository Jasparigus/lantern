#!/usr/bin/env bash
# Downloads pinned static ffmpeg/ffprobe builds into src-tauri/binaries/ with
# the target-triple suffix Tauri's externalBin expects:
#   ffmpeg-<triple>[.exe], ffprobe-<triple>[.exe]
#
# usage: scripts/prepare-sidecars.sh [target-triple]
#   (defaults to the host triple)
#
# Sources:
#   linux   — https://github.com/BtbN/FFmpeg-Builds  (GPL builds incl. libx264)
#   windows — https://github.com/BtbN/FFmpeg-Builds
#   macos   — https://github.com/eugeneware/ffmpeg-static
set -euo pipefail

TRIPLE="${1:-$(rustc -Vv | sed -n 's/^host: //p')}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DEST="$ROOT/src-tauri/binaries"
mkdir -p "$DEST"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

fetch() {
  echo "fetching $1"
  curl -fL --retry 3 -o "$TMP/download" "$1"
}

echo "preparing ffmpeg sidecars for: $TRIPLE"

case "$TRIPLE" in
  *-linux-*)
    case "$TRIPLE" in
      aarch64*) VARIANT=linuxarm64 ;;
      *)        VARIANT=linux64 ;;
    esac
    fetch "https://github.com/BtbN/FFmpeg-Builds/releases/latest/download/ffmpeg-master-latest-${VARIANT}-gpl.tar.xz"
    tar -xJf "$TMP/download" -C "$TMP"
    SRC="$TMP/ffmpeg-master-latest-${VARIANT}-gpl/bin"
    install -m 0755 "$SRC/ffmpeg"  "$DEST/ffmpeg-$TRIPLE"
    install -m 0755 "$SRC/ffprobe" "$DEST/ffprobe-$TRIPLE"
    ;;
  *-windows-*)
    fetch "https://github.com/BtbN/FFmpeg-Builds/releases/latest/download/ffmpeg-master-latest-win64-gpl.zip"
    unzip -q -o "$TMP/download" -d "$TMP"
    SRC="$TMP/ffmpeg-master-latest-win64-gpl/bin"
    install -m 0755 "$SRC/ffmpeg.exe"  "$DEST/ffmpeg-$TRIPLE.exe"
    install -m 0755 "$SRC/ffprobe.exe" "$DEST/ffprobe-$TRIPLE.exe"
    ;;
  *-apple-darwin)
    case "$TRIPLE" in
      aarch64*)
        fetch "https://github.com/eugeneware/ffmpeg-static/releases/latest/download/ffmpeg-darwin-arm64"
        install -m 0755 "$TMP/download" "$DEST/ffmpeg-$TRIPLE"
        fetch "https://github.com/eugeneware/ffmpeg-static/releases/latest/download/ffprobe-darwin-arm64"
        install -m 0755 "$TMP/download" "$DEST/ffprobe-$TRIPLE"
        ;;
      universal-apple-darwin)
        fetch "https://github.com/eugeneware/ffmpeg-static/releases/latest/download/ffmpeg-darwin-arm64"
        mv "$TMP/download" "$TMP/ffmpeg-arm64"
        fetch "https://github.com/eugeneware/ffmpeg-static/releases/latest/download/ffmpeg-darwin-x64"
        mv "$TMP/download" "$TMP/ffmpeg-x64"
        fetch "https://github.com/eugeneware/ffmpeg-static/releases/latest/download/ffprobe-darwin-arm64"
        mv "$TMP/download" "$TMP/ffprobe-arm64"
        fetch "https://github.com/eugeneware/ffmpeg-static/releases/latest/download/ffprobe-darwin-x64"
        mv "$TMP/download" "$TMP/ffprobe-x64"
        lipo -create "$TMP/ffmpeg-arm64" "$TMP/ffmpeg-x64" -output "$DEST/ffmpeg-$TRIPLE"
        lipo -create "$TMP/ffprobe-arm64" "$TMP/ffprobe-x64" -output "$DEST/ffprobe-$TRIPLE"
        chmod 0755 "$DEST/ffmpeg-$TRIPLE" "$DEST/ffprobe-$TRIPLE"
        ;;
      *)
        fetch "https://github.com/eugeneware/ffmpeg-static/releases/latest/download/ffmpeg-darwin-x64"
        install -m 0755 "$TMP/download" "$DEST/ffmpeg-$TRIPLE"
        fetch "https://github.com/eugeneware/ffmpeg-static/releases/latest/download/ffprobe-darwin-x64"
        install -m 0755 "$TMP/download" "$DEST/ffprobe-$TRIPLE"
        ;;
    esac
    ;;
  *)
    echo "unsupported target triple: $TRIPLE" >&2
    exit 1
    ;;
esac

# macOS Gatekeeper quarantines downloaded binaries; drop it.
case "$TRIPLE" in
  *-apple-darwin) xattr -d com.apple.quarantine "$DEST"/ffmpeg-$TRIPLE "$DEST"/ffprobe-$TRIPLE 2>/dev/null || true ;;
esac

echo "done:"
ls -la "$DEST"
