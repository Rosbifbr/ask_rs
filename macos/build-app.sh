#!/bin/sh
# Assemble macos/Ask.app — a LSUIElement HUD that execs the bundled ask binary.
set -eu

ROOT=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
MACOS="$ROOT/macos"
APP="$MACOS/Ask.app"

echo "==> building ask (release)"
cargo build --release --manifest-path "$ROOT/Cargo.toml"

echo "==> building Ask host (release)"
swift build -c release --package-path "$MACOS"
BIN_DIR=$(swift build -c release --package-path "$MACOS" --show-bin-path)

if [ ! -x "$BIN_DIR/AskApp" ]; then
    echo "error: AskApp binary not found at $BIN_DIR/AskApp" >&2
    exit 1
fi

if [ ! -x "$ROOT/target/release/ask_rs" ]; then
    echo "error: ask_rs binary not found at $ROOT/target/release/ask_rs" >&2
    exit 1
fi

echo "==> assembling $APP"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Helpers" "$APP/Contents/Resources"
# The host must be named `Ask` to match CFBundleExecutable. The CLI cannot
# be `ask` — APFS is case-insensitive, so that would overwrite the host.
cp "$BIN_DIR/AskApp" "$APP/Contents/MacOS/Ask"
cp "$ROOT/target/release/ask_rs" "$APP/Contents/Helpers/ask"
cp "$MACOS/Info.plist" "$APP/Contents/Info.plist"
printf 'APPL????' > "$APP/Contents/PkgInfo"
chmod +x "$APP/Contents/MacOS/Ask" "$APP/Contents/Helpers/ask"

echo "==> ad-hoc codesign"
codesign --force --deep --sign - "$APP"

echo
echo "built $APP"
echo "  launch:    open \"$APP\""
echo "  spotlight: copy to /Applications first:"
echo "             cp -R \"$APP\" /Applications/"
echo "  raycast:   open 'ask://q?text=hello'"
