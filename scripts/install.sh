#!/usr/bin/env bash
# Installs miz for the current user only (no sudo):
#   ~/.local/bin/miz
#   ~/.local/share/applications/com.antonjoy.miz.desktop
#   ~/.local/share/icons/hicolor/scalable/apps/com.antonjoy.miz.svg
#   ~/.local/share/metainfo/com.antonjoy.miz.metainfo.xml
# Usage: scripts/install.sh [--uninstall]
set -euo pipefail

cd "$(dirname "$0")/.."
APP_ID=com.antonjoy.miz
BIN_DIR="$HOME/.local/bin"
APP_DIR="$HOME/.local/share/applications"
ICON_DIR="$HOME/.local/share/icons/hicolor/scalable/apps"
META_DIR="$HOME/.local/share/metainfo"

# Files from versions before the app ID rename.
rm -f "$APP_DIR/miz.desktop" "$ICON_DIR/miz.svg"

if [[ "${1:-}" == "--uninstall" ]]; then
    rm -f "$BIN_DIR/miz" "$APP_DIR/$APP_ID.desktop" "$ICON_DIR/$APP_ID.svg" \
        "$META_DIR/$APP_ID.metainfo.xml"
    command -v update-desktop-database >/dev/null && update-desktop-database "$APP_DIR" || true
    echo "miz uninstalled (your config in ~/.config/miz is kept)."
    exit 0
fi

cargo build --release
install -Dm755 target/release/miz "$BIN_DIR/miz"
install -Dm644 "assets/$APP_ID.svg" "$ICON_DIR/$APP_ID.svg"
install -Dm644 "assets/$APP_ID.metainfo.xml" "$META_DIR/$APP_ID.metainfo.xml"
mkdir -p "$APP_DIR"
sed "s|^Exec=miz|Exec=$BIN_DIR/miz|" "assets/$APP_ID.desktop" > "$APP_DIR/$APP_ID.desktop"
chmod 644 "$APP_DIR/$APP_ID.desktop"
# Refresh the launcher's caches (both are per-user here).
command -v update-desktop-database >/dev/null && update-desktop-database "$APP_DIR" || true
command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q -t "$HOME/.local/share/icons/hicolor" || true

echo "Installed $BIN_DIR/miz"
case ":$PATH:" in
    *":$BIN_DIR:"*) ;;
    *) echo "Note: $BIN_DIR is not in your PATH; the app launcher entry works regardless." ;;
esac
