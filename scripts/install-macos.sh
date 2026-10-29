#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Build WowUp.rs and install it as /Applications/WowUp.rs.app (TUI launcher + wowup:// handler),
# plus the `wowup` CLI and `wowup-tui` on your PATH.
#
#   scripts/install-macos.sh [--prefix /Applications] [--bin-dir ~/.local/bin]
set -euo pipefail

APP_DIR="/Applications"
BIN_DIR="$HOME/.local/bin"
while [[ $# -gt 0 ]]; do
  case "$1" in
    --prefix) APP_DIR="$2"; shift 2 ;;
    --bin-dir) BIN_DIR="$2"; shift 2 ;;
    *) echo "unknown option $1" >&2; exit 1 ;;
  esac
done

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
APP="$APP_DIR/WowUp.rs.app"
VERSION="$(grep -m1 '^version' "$ROOT/Cargo.toml" | cut -d'"' -f2)"

echo "==> Building release binaries"
cargo build --release --manifest-path "$ROOT/Cargo.toml" -p wowup-cli -p wowup-tui

echo "==> Creating $APP"
STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
osacompile -o "$STAGE/WowUp.rs.app" "$ROOT/packaging/macos/launcher.applescript"
RES="$STAGE/WowUp.rs.app/Contents/Resources"
mkdir -p "$RES/bin"
cp "$ROOT/target/release/wowup-tui" "$ROOT/target/release/wowup" "$RES/bin/"
cp "$ROOT/packaging/macos/WowUp.icns" "$RES/applet.icns"
cp "$ROOT/LICENSE" "$RES/LICENSE"

PLIST="$STAGE/WowUp.rs.app/Contents/Info.plist"
pb() { /usr/libexec/PlistBuddy -c "$1" "$PLIST" >/dev/null 2>&1 || true; }
pb "Set :CFBundleIdentifier io.wowup.rs"
pb "Add :CFBundleIdentifier string io.wowup.rs"
pb "Set :CFBundleName WowUp.rs"
pb "Add :CFBundleShortVersionString string $VERSION"
pb "Set :CFBundleShortVersionString $VERSION"
pb "Add :NSHumanReadableCopyright string GNU GPL v3 or later"
pb "Delete :CFBundleURLTypes"
pb "Add :CFBundleURLTypes array"
pb "Add :CFBundleURLTypes:0 dict"
pb "Add :CFBundleURLTypes:0:CFBundleURLName string WowUp addon link"
pb "Add :CFBundleURLTypes:0:CFBundleURLSchemes array"
pb "Add :CFBundleURLTypes:0:CFBundleURLSchemes:0 string wowup"
pb "Add :CFBundleURLTypes:0:CFBundleURLSchemes:1 string curseforge"

# Ad-hoc sign so Gatekeeper accepts the locally built bundle.
codesign --force --deep --sign - "$STAGE/WowUp.rs.app" >/dev/null 2>&1 || true

if [[ -e "$APP" ]]; then
  echo "    replacing existing $APP"
  rm -rf "$APP"
fi
mv "$STAGE/WowUp.rs.app" "$APP"
/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -f "$APP" || true

echo "==> Linking CLI into $BIN_DIR"
mkdir -p "$BIN_DIR"
ln -sf "$APP/Contents/Resources/bin/wowup" "$BIN_DIR/wowup"
ln -sf "$APP/Contents/Resources/bin/wowup-tui" "$BIN_DIR/wowup-tui"
case ":$PATH:" in
  *":$BIN_DIR:"*) ;;
  *) echo "    note: add $BIN_DIR to your PATH to use \`wowup\` and \`wowup-tui\` from a shell" ;;
esac

echo "==> Installed WowUp.rs $VERSION"
echo "    Open it from Launchpad/Spotlight, or run: open -a WowUp.rs"
