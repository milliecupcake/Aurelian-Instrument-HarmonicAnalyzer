// アプリのアイコン（1024×1024 の PNG）を描く。scripts/make_icon.sh から呼ばれる。
// 模様は倍音分布の棒グラフ。色はアプリの倍音の色（src/main.rs の PALETTE）と同じ。
import AppKit

let out = CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : "icon_1024.png"
let size = 1024
let rep = NSBitmapImageRep(
    bitmapDataPlanes: nil, pixelsWide: size, pixelsHigh: size, bitsPerSample: 8,
    samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB,
    bytesPerRow: 0, bitsPerPixel: 0)!
NSGraphicsContext.saveGraphicsState()
NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)

func rgb(_ r: CGFloat, _ g: CGFloat, _ b: CGFloat, _ a: CGFloat = 1) -> NSColor {
    NSColor(deviceRed: r / 255, green: g / 255, blue: b / 255, alpha: a)
}

// macOS のアイコンの枠（1024 の中に 824 の角丸、上下左右に余白）
let plate = NSRect(x: 100, y: 100, width: 824, height: 824)
let platePath = NSBezierPath(roundedRect: plate, xRadius: 185, yRadius: 185)
NSGraphicsContext.current!.saveGraphicsState()
let shadow = NSShadow()
shadow.shadowColor = rgb(0, 0, 0, 0.45)
shadow.shadowOffset = NSSize(width: 0, height: -12)
shadow.shadowBlurRadius = 24
shadow.set()
rgb(22, 24, 30).setFill()
platePath.fill()
NSGraphicsContext.current!.restoreGraphicsState()
NSGradient(starting: rgb(40, 44, 56), ending: rgb(14, 15, 20))!.draw(in: platePath, angle: -90)

platePath.addClip()

// 目盛りの横線
rgb(255, 255, 255, 0.07).setStroke()
for i in 1...5 {
    let y = plate.minY + 150 + CGFloat(i) * 110
    let line = NSBezierPath()
    line.move(to: NSPoint(x: plate.minX, y: y))
    line.line(to: NSPoint(x: plate.maxX, y: y))
    line.lineWidth = 4
    line.stroke()
}

// 倍音の棒（第1〜第8倍音、高い倍音ほど弱い）
let colors = [
    rgb(86, 156, 255), rgb(255, 99, 99), rgb(120, 210, 90), rgb(220, 120, 255),
    rgb(255, 200, 60), rgb(60, 210, 210), rgb(255, 140, 190), rgb(170, 170, 255),
]
let heights: [CGFloat] = [1.0, 0.78, 0.86, 0.55, 0.62, 0.38, 0.44, 0.26]
let base = plate.minY + 150
let maxH: CGFloat = 560
let slot: CGFloat = 76
let barW: CGFloat = 50
let left = plate.midX - slot * 4 + (slot - barW) / 2
for (i, h) in heights.enumerated() {
    let r = NSRect(x: left + CGFloat(i) * slot, y: base, width: barW, height: maxH * h)
    colors[i].setFill()
    NSBezierPath(roundedRect: r, xRadius: 10, yRadius: 10).fill()
}

// 基線
rgb(255, 255, 255, 0.35).setFill()
NSBezierPath(rect: NSRect(x: plate.minX + 90, y: base - 8, width: plate.width - 180, height: 8)).fill()

NSGraphicsContext.restoreGraphicsState()
try! rep.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: out))
print("描きました: \(out)")
