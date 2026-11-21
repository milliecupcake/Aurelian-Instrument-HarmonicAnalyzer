#!/bin/bash
# macOS 用の .app を作る。
# アプリの音の取り込み（Core Audio プロセスタップ）には「システムオーディオ録音」の許可が要り、
# その許可は Info.plist に NSAudioCaptureUsageDescription を持つアプリにしか求められないため。
#
#   ./scripts/bundle_macos.sh            target/release/HarmonicAnalyzer.app を作る
#   ./scripts/bundle_macos.sh --install  作った上で /Applications に置く（書き込めなければ ~/Applications）
set -euo pipefail
cd "$(dirname "$0")/.."
INSTALL=0
case "${1:-}" in
  --install) INSTALL=1 ;;
  "") ;;
  *) echo "使い方: $0 [--install]" >&2; exit 2 ;;
esac
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
cargo build --release
APP="target/release/HarmonicAnalyzer.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp target/release/harmonic_analyzer "$APP/Contents/MacOS/harmonic_analyzer"
cp assets/AppIcon.icns "$APP/Contents/Resources/AppIcon.icns"
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>倍音解析エディタ</string>
  <key>CFBundleDisplayName</key><string>倍音解析エディタ</string>
  <key>CFBundleIdentifier</key><string>com.aurelian-instrument.harmonic-analyzer</string>
  <key>CFBundleExecutable</key><string>harmonic_analyzer</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleIconFile</key><string>AppIcon</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleVersion</key><string>1</string>
  <key>LSMinimumSystemVersion</key><string>14.2</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSMicrophoneUsageDescription</key><string>ライブ入力で入力デバイスの音を解析するために使います。</string>
  <key>NSAudioCaptureUsageDescription</key><string>選んだアプリ（ブラウザなど）が出している音を解析するために使います。</string>
</dict>
</plist>
PLIST
# 許可を同じアプリとして覚えてもらうため、アドホック署名をする
codesign --force --deep --sign - "$APP"
echo "作成しました: $APP"

if [ "$INSTALL" = 1 ]; then
  DEST_DIR=/Applications
  [ -w "$DEST_DIR" ] || { DEST_DIR="$HOME/Applications"; mkdir -p "$DEST_DIR"; }
  DEST="$DEST_DIR/HarmonicAnalyzer.app"
  if pgrep -f "$DEST/Contents/MacOS/" >/dev/null; then
    echo "$DEST が起動中です。終了してからもう一度実行してください。" >&2
    exit 1
  fi
  rm -rf "$DEST"
  ditto "$APP" "$DEST"
  # Finder・Dock・Spotlight にアイコンと名前を読み直してもらう
  touch "$DEST"
  /System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -f "$DEST" || true
  echo "配置しました: $DEST"
fi
