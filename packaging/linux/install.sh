#!/usr/bin/env sh
# Install RustCord for the current user (no root needed):
#   ~/.local/bin/rustcord, a menu entry and the icon.
# Run from the unpacked release folder:  ./install.sh
# Undo with:  ./install.sh --uninstall
set -eu

here=$(cd "$(dirname "$0")" && pwd)
bin_dir="${XDG_BIN_HOME:-$HOME/.local/bin}"
data_dir="${XDG_DATA_HOME:-$HOME/.local/share}"

if [ "${1:-}" = "--uninstall" ]; then
    rm -f "$bin_dir/rustcord" \
        "$data_dir/applications/rustcord.desktop" \
        "$data_dir/icons/hicolor/256x256/apps/rustcord.png"
    echo "RustCord removed (your settings and saved login are kept)."
    exit 0
fi

mkdir -p "$bin_dir" "$data_dir/applications" "$data_dir/icons/hicolor/256x256/apps"
install -m 755 "$here/rustcord" "$bin_dir/rustcord"
install -m 644 "$here/rustcord.png" "$data_dir/icons/hicolor/256x256/apps/rustcord.png"
sed "s|^Exec=rustcord|Exec=$bin_dir/rustcord|" "$here/rustcord.desktop" \
    > "$data_dir/applications/rustcord.desktop"
echo "RustCord installed to $bin_dir/rustcord (find it in your app menu)."
