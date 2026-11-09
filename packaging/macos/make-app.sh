#!/usr/bin/env bash
# Wrap a built rustcord binary into a minimal RustCord.app bundle.
#
#   packaging/macos/make-app.sh <path/to/rustcord> <version> <out-dir>
#
# Produces <out-dir>/RustCord.app (Info.plist, icon, LICENSE, README), signed
# ad hoc (codesign -s -) when codesign is available. It is NOT notarised, so
# macOS asks once before the first launch (see docs/RELEASING.md).
set -euo pipefail

bin=$1
version=$2
out=$3
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)

app="$out/RustCord.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"

cp "$bin" "$app/Contents/MacOS/rustcord"
chmod +x "$app/Contents/MacOS/rustcord"
cp "$root/assets/icon/rustcord.icns" "$app/Contents/Resources/rustcord.icns"
cp "$root/LICENSE" "$app/Contents/Resources/LICENSE"
cp "$root/README.md" "$app/Contents/Resources/README.md"
sed "s/@VERSION@/$version/g" "$here/Info.plist" > "$app/Contents/Info.plist"
printf 'APPL????' > "$app/Contents/PkgInfo"

if command -v plutil >/dev/null; then
    plutil -lint "$app/Contents/Info.plist"
fi
if command -v codesign >/dev/null; then
    codesign --force --deep --sign - "$app"
    codesign --verify --verbose "$app"
fi
echo "built $app"
