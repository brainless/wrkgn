// Generator for src/og-image.png, the maintainer-approved 1200x630 Open Graph preview image.
// Regenerate on macOS with:  swift og-image.swift <out.png>
// It uses Helvetica Neue Bold because CoreText cannot load the WOFF2 web fonts.
// The output is checked in as ../src/og-image.png; no build or test runs this script.
import CoreGraphics
import CoreText
import ImageIO
import Foundation
import UniformTypeIdentifiers
let W = 1200, H = 630
let cs = CGColorSpace(name: CGColorSpace.sRGB)!
let ctx = CGContext(data: nil, width: W, height: H, bitsPerComponent: 8, bytesPerRow: 0, space: cs, bitmapInfo: CGImageAlphaInfo.noneSkipLast.rawValue)!
func col(_ h: UInt32, _ a: CGFloat = 1) -> CGColor {
  CGColor(colorSpace: cs, components: [CGFloat((h>>16)&255)/255, CGFloat((h>>8)&255)/255, CGFloat(h&255)/255, a])!
}
let green: UInt32 = 0x264c3d, cream: UInt32 = 0xf7f6f0, orange: UInt32 = 0xc96138
ctx.setFillColor(col(green)); ctx.fill(CGRect(x: 0, y: 0, width: W, height: H))
func font(_ size: CGFloat) -> CTFont { CTFontCreateWithName("HelveticaNeue-Bold" as CFString, size, nil) }
func line(_ s: String, _ f: CTFont, _ c: CGColor, kern: CGFloat = 0) -> CTLine {
  let a = NSAttributedString(string: s, attributes: [NSAttributedString.Key(kCTFontAttributeName as String): f, NSAttributedString.Key(kCTForegroundColorAttributeName as String): c, NSAttributedString.Key(kCTKernAttributeName as String): kern])
  return CTLineCreateWithAttributedString(a)
}
// Icon: cream rounded square, green w, orange dot
let icon: CGFloat = 220
let cx = CGFloat(W)/2
let gap: CGFloat = 56
let wm = line("wrkgn", font(190), col(cream), kern: -4)
let wmB = CTLineGetBoundsWithOptions(wm, .useGlyphPathBounds)
let total = icon + gap + wmB.width
let x0 = (CGFloat(W) - total)/2
let rowMid: CGFloat = 340
let ir = CGRect(x: x0, y: rowMid - icon/2, width: icon, height: icon)
ctx.setFillColor(col(cream))
ctx.addPath(CGPath(roundedRect: ir, cornerWidth: icon*7/32, cornerHeight: icon*7/32, transform: nil)); ctx.fillPath()
let wl = line("w", font(icon*25/32*1.0), col(green))
let wb = CTLineGetBoundsWithOptions(wl, .useGlyphPathBounds)
ctx.textPosition = CGPoint(x: ir.minX + icon*0.17 - wb.minX, y: ir.minY + icon*(1-24.0/32) )
CTLineDraw(wl, ctx)
ctx.setFillColor(col(orange))
let r = icon*3/32
ctx.fillEllipse(in: CGRect(x: ir.minX + icon*26/32 - r, y: ir.minY + icon*(1-25.0/32) - r, width: 2*r, height: 2*r))
// wordmark, vertically centred on icon (x-height-ish: use bounds)
ctx.textPosition = CGPoint(x: x0 + icon + gap - wmB.minX, y: rowMid - wmB.height/2 - wmB.minY)
CTLineDraw(wm, ctx)
// tagline
let tl = line("Projects shared on Hacker News", font(44), col(cream, 0.78), kern: 0.5)
let tb = CTLineGetBoundsWithOptions(tl, .useGlyphPathBounds)
ctx.textPosition = CGPoint(x: (CGFloat(W)-tb.width)/2 - tb.minX, y: 150)
CTLineDraw(tl, ctx)
// orange accent bar
ctx.setFillColor(col(orange))
ctx.fill(CGRect(x: cx-40, y: 215, width: 80, height: 6))
let img = ctx.makeImage()!
let url = URL(fileURLWithPath: CommandLine.arguments[1]) as CFURL
let d = CGImageDestinationCreateWithURL(url, UTType.png.identifier as CFString, 1, nil)!
CGImageDestinationAddImage(d, img, nil); CGImageDestinationFinalize(d)
