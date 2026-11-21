#!/bin/bash
# assets/AppIcon.icns を作り直す（アイコンの絵を変えたときだけ実行する）。
set -euo pipefail
cd "$(dirname "$0")/.."
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
swift scripts/make_icon.swift "$WORK/icon_1024.png"
SET="$WORK/AppIcon.iconset"
mkdir -p "$SET"
for s in 16 32 128 256 512; do
  sips -z $s $s "$WORK/icon_1024.png" --out "$SET/icon_${s}x${s}.png" >/dev/null
  sips -z $((s * 2)) $((s * 2)) "$WORK/icon_1024.png" --out "$SET/icon_${s}x${s}@2x.png" >/dev/null
done
mkdir -p assets
iconutil -c icns "$SET" -o assets/AppIcon.icns
echo "作成しました: assets/AppIcon.icns"
