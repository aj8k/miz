#!/usr/bin/env bash
# Installs miz for the current user only (no sudo):
#   ~/.local/bin/miz
#   ~/.local/share/applications/miz.desktop
#   ~/.local/share/icons/hicolor/scalable/apps/miz.svg
# Usage: scripts/install.sh [--uninstall]
set -euo pipefail

cd "$(dirname "$0")/.."
BIN_DIR="$HOME/.local/bin"
APP_DIR="$HOME/.local/share/applications"
ICON_DIR="$HOME/.local/share/icons/hicolor/scalable/apps"

if [[ "${1:-}" == "--uninstall" ]]; then
    rm -f "$BIN_DIR/miz" "$APP_DIR/miz.desktop" "$ICON_DIR/miz.svg"
    command -v update-desktop-database >/dev/null && update-desktop-database "$APP_DIR" || true
    echo "miz uninstalled (your config in ~/.config/miz is kept)."
    exit 0
fi

cargo build --release
install -Dm755 target/release/miz "$BIN_DIR/miz"
install -Dm644 assets/miz.svg "$ICON_DIR/miz.svg"
mkdir -p "$APP_DIR"
sed "s|@BINDIR@|$BIN_DIR|" assets/miz.desktop > "$APP_DIR/miz.desktop"
chmod 644 "$APP_DIR/miz.desktop"
# Refresh the launcher's caches (both are per-user here).
command -v update-desktop-database >/dev/null && update-desktop-database "$APP_DIR" || true
command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q -t "$HOME/.local/share/icons/hicolor" || true

echo "Installed $BIN_DIR/miz"
case ":$PATH:" in
    *":$BIN_DIR:"*) ;;
    *) echo "Note: $BIN_DIR is not in your PATH; the app launcher entry works regardless." ;;
esac
