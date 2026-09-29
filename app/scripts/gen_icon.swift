// 生成 StoryStudio 水墨风应用图标（1024×1024）
// 用法: swift gen_icon.swift <输出路径>
import Foundation
import CoreGraphics
import CoreText
import ImageIO
import UniformTypeIdentifiers

let size = 1024
let out = CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : "app-icon.png"

let colorSpace = CGColorSpace(name: CGColorSpace.sRGB)!
let ctx = CGContext(data: nil, width: size, height: size, bitsPerComponent: 8,
                    bytesPerRow: 0, space: colorSpace,
                    bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!

// 宣纸底色（暖米白）
ctx.setFillColor(CGColor(srgbRed: 0.949, green: 0.941, blue: 0.918, alpha: 1))
ctx.fill(CGRect(x: 0, y: 0, width: size, height: size))

// 纸张微妙渐晕（四角略暗）
let gradient = CGGradient(colorsSpace: colorSpace, colors: [
    CGColor(srgbRed: 0.90, green: 0.88, blue: 0.84, alpha: 1),
    CGColor(srgbRed: 0.949, green: 0.941, blue: 0.918, alpha: 1)
] as CFArray, locations: [0, 1])!
ctx.drawRadialGradient(gradient, startCenter: CGPoint(x: 512, y: 512), startRadius: 200,
                       endCenter: CGPoint(x: 512, y: 512), endRadius: 760, options: [])

// 噪点颗粒（纸纹）
srand48(42)
ctx.setFillColor(CGColor(srgbRed: 0.75, green: 0.72, blue: 0.67, alpha: 1))
for _ in 0..<2600 {
    let x = CGFloat(drand48()) * 1024
    let y = CGFloat(drand48()) * 1024
    let r = CGFloat(drand48()) * 1.4 + 0.3
    ctx.fillEllipse(in: CGRect(x: x, y: y, width: r, height: r))
}

func drawText(_ text: String, fontName: String, fontSize: CGFloat,
              center: CGPoint, color: CGColor) {
    let font = CTFontCreateWithName(fontName as CFString, fontSize, nil)
    let attrs: [CFString: Any] = [
        kCTFontAttributeName: font,
        kCTForegroundColorAttributeName: color
    ]
    let attrString = CFAttributedStringCreate(nil, text as CFString, attrs as CFDictionary)!
    let line = CTLineCreateWithAttributedString(attrString)
    let bounds = CTLineGetBoundsWithOptions(line, .useOpticalBounds)
    ctx.textPosition = CGPoint(x: center.x - bounds.width / 2 - bounds.minX,
                               y: center.y - bounds.height / 2 - bounds.minY)
    CTLineDraw(line, ctx)
}

// 主墨字「书」— 墨色
let ink = CGColor(srgbRed: 0.141, green: 0.122, blue: 0.106, alpha: 1)
drawText("书", fontName: "Songti SC Bold", fontSize: 560, center: CGPoint(x: 512, y: 560), color: ink)

// 朱印（右下角暗红印章 + 空心小字）
let seal = CGRect(x: 700, y: 130, width: 170, height: 170)
let sealPath = CGPath(roundedRect: seal, cornerWidth: 26, cornerHeight: 26, transform: nil)
ctx.setFillColor(CGColor(srgbRed: 0.478, green: 0.188, blue: 0.157, alpha: 1))
ctx.addPath(sealPath)
ctx.fillPath()
// 印章内空心「事」
drawText("事", fontName: "Songti SC Bold", fontSize: 108,
         center: CGPoint(x: 785, y: 215),
         color: CGColor(srgbRed: 0.949, green: 0.941, blue: 0.918, alpha: 1))

// 左侧细墨线（装订线意象）
ctx.setStrokeColor(CGColor(srgbRed: 0.141, green: 0.122, blue: 0.106, alpha: 0.55))
ctx.setLineWidth(7)
ctx.setLineCap(.round)
ctx.move(to: CGPoint(x: 118, y: 210))
ctx.addLine(to: CGPoint(x: 118, y: 810))
ctx.strokePath()

let image = ctx.makeImage()!
let url = URL(fileURLWithPath: out) as CFURL
let dest = CGImageDestinationCreateWithURL(url, UTType.png.identifier as CFString, 1, nil)!
CGImageDestinationAddImage(dest, image, nil)
CGImageDestinationFinalize(dest)
print("written: \(out)")
