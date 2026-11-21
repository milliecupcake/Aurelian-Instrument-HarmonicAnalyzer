#!/bin/bash
# macOS 用の .app を作る。
# アプリの音の取り込み（Core Audio プロセスタップ）には「システムオーディオ録音」の許可が要り、
# その許可は Info.plist に NSAudioCaptureUsageDescription を持つアプリにしか求められないため。
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build --release
APP="target/release/HarmonicAnalyzer.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS"
cp target/release/harmonic_analyzer "$APP/Contents/MacOS/harmonic_analyzer"
cat > "$APP/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>倍音解析エディタ</string>
  <key>CFBundleDisplayName</key><string>倍音解析エディタ</string>
  <key>CFBundleIdentifier</key><string>com.aurelian-instrument.harmonic-analyzer</string>
  <key>CFBundleExecutable</key><string>harmonic_analyzer</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>0.1.0</string>
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
