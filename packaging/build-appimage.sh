#!/usr/bin/env bash
# Build US_Explorer-<version>-x86_64.AppImage from release binaries.
# Usage: packaging/build-appimage.sh <target-dir-with-binaries> <version> <out-dir>
set -euo pipefail
BIN_DIR=$1; VERSION=$2; OUT=${3:-dist}
HERE=$(cd "$(dirname "$0")" && pwd)
WORK=$(mktemp -d)
APPDIR=$WORK/AppDir
mkdir -p "$APPDIR/usr/bin" "$APPDIR/usr/share/applications" "$APPDIR/usr/share/icons/hicolor/scalable/apps" "$OUT"
install -m755 "$BIN_DIR/us-explorer" "$APPDIR/usr/bin/"
cp "$HERE/us-explorer.desktop" "$APPDIR/" && cp "$HERE/us-explorer.desktop" "$APPDIR/usr/share/applications/"
cp "$HERE/us-explorer.svg" "$APPDIR/us-explorer.svg" && cp "$HERE/us-explorer.svg" "$APPDIR/usr/share/icons/hicolor/scalable/apps/"
cat > "$APPDIR/AppRun" <<'RUN'
#!/bin/sh
HERE=$(dirname "$(readlink -f "$0")")
exec "$HERE/usr/bin/us-explorer" "$@"
RUN
chmod +x "$APPDIR/AppRun"
TOOL=$WORK/appimagetool
curl -fsSL -o "$TOOL" https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-x86_64.AppImage
chmod +x "$TOOL"
NAME="US_Explorer-${VERSION}-x86_64.AppImage"
# The continuous appimagetool embeds the static type-2 runtime, so the result runs
# on Fedora 40+ without libfuse2.
ARCH=x86_64 "$TOOL" --appimage-extract-and-run --comp zstd "$APPDIR" "$OUT/$NAME"
(cd "$OUT" && sha256sum "$NAME" > "$NAME.sha256")
ls -l "$OUT"
