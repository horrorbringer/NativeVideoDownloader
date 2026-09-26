import Foundation
import CoreGraphics
import ImageIO

let args = CommandLine.arguments
guard args.count >= 3 else {
    print("Usage: make_transparent_icon <input_jpg> <output_png>")
    exit(1)
}

let inputPath = args[1]
let outputPath = args[2]

guard let url = CFURLCreateWithFileSystemPath(nil, inputPath as CFString, .cfurlposixPathStyle, false),
      let src = CGImageSourceCreateWithURL(url, nil),
      let cgImage = CGImageSourceCreateImageAtIndex(src, 0, nil) else {
    print("Error loading input image: \(inputPath)")
    exit(1)
}

let width = cgImage.width
let height = cgImage.height
var rawData = [UInt8](repeating: 0, count: width * height * 4)
let colorSpace = CGColorSpaceCreateDeviceRGB()
guard let context = CGContext(data: &rawData, width: width, height: height, bitsPerComponent: 8, bytesPerRow: width * 4, space: colorSpace, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else { exit(1) }
context.draw(cgImage, in: CGRect(x: 0, y: 0, width: width, height: height))

var outData = [UInt8](repeating: 0, count: width * height * 4)

let cornerR = 175.0
let tl = (x: 130.0 + cornerR, y: 126.0 + cornerR)
let tr = (x: 894.0 - cornerR, y: 126.0 + cornerR)
let bl = (x: 130.0 + cornerR, y: 890.0 - cornerR)
let br = (x: 894.0 - cornerR, y: 890.0 - cornerR)

func isOutsideIconAndShadow(x: Int, y: Int) -> Bool {
    let px = Double(x)
    let py = Double(y)
    
    if px < 75 || px > 949 || py < 115 || py > 990 { return true }
    
    let shadowAllowance = 25.0
    let maxR = cornerR + shadowAllowance
    
    if px < tl.x && py < tl.y {
        let d = hypot(px - tl.x, py - tl.y)
        if d > maxR { return true }
    }
    if px > tr.x && py < tr.y {
        let d = hypot(px - tr.x, py - tr.y)
        if d > maxR { return true }
    }
    if px < bl.x && py > bl.y {
        let d = hypot(px - bl.x, py - bl.y)
        if d > (maxR + 15.0) { return true }
    }
    if px > br.x && py > br.y {
        let d = hypot(px - br.x, py - br.y)
        if d > (maxR + 15.0) { return true }
    }
    
    return false
}

func clamp(_ v: Double, _ minV: Double, _ maxV: Double) -> Double {
    return max(minV, min(maxV, v))
}

for y in 0..<height {
    for x in 0..<width {
        let off = (y * width + x) * 4
        
        if isOutsideIconAndShadow(x: x, y: y) {
            outData[off] = 0
            outData[off + 1] = 0
            outData[off + 2] = 0
            outData[off + 3] = 0
            continue
        }
        
        let r = Double(rawData[off])
        let g = Double(rawData[off + 1])
        let b = Double(rawData[off + 2])
        let minC = min(r, min(g, b))
        let maxC = max(r, max(g, b))
        
        let dx = Double(x - 512)
        let dy = Double(y - 508)
        let dist = sqrt(dx * dx + dy * dy)
        
        if dist < 320 {
            outData[off] = rawData[off]
            outData[off + 1] = rawData[off + 1]
            outData[off + 2] = rawData[off + 2]
            outData[off + 3] = 255
            continue
        }
        
        if minC > 248 {
            outData[off] = 0
            outData[off + 1] = 0
            outData[off + 2] = 0
            outData[off + 3] = 0
        } else {
            let isOutsideSquircle = (x < 130 || x > 894 || y < 126 || y > 890)
            let isGrayscale = (maxC - minC) < 15
            
            if isOutsideSquircle && isGrayscale {
                let avgC = (r + g + b) / 3.0
                let a = max(0.0, (250.0 - avgC) / 250.0)
                let aByte = UInt8(clamp(a * 0.9 * 255.0, 0, 255))
                outData[off] = 0
                outData[off + 1] = 0
                outData[off + 2] = 0
                outData[off + 3] = aByte
            } else {
                let a = min(1.0, max(0.0, 1.0 - (minC / 250.0) * 0.95))
                if a > 0.08 {
                    let invA = 1.0 / a
                    let fgR = clamp((r - 250.0 * (1.0 - a)) * invA, 0, 255)
                    let fgG = clamp((g - 250.0 * (1.0 - a)) * invA, 0, 255)
                    let fgB = clamp((b - 250.0 * (1.0 - a)) * invA, 0, 255)
                    outData[off] = UInt8(fgR)
                    outData[off + 1] = UInt8(fgG)
                    outData[off + 2] = UInt8(fgB)
                    outData[off + 3] = UInt8(clamp(a * 255.0, 0, 255))
                } else {
                    outData[off] = 0
                    outData[off + 1] = 0
                    outData[off + 2] = 0
                    outData[off + 3] = 0
                }
            }
        }
    }
}

guard let outContext = CGContext(data: &outData, width: width, height: height, bitsPerComponent: 8, bytesPerRow: width * 4, space: colorSpace, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue),
      let outCgImage = outContext.makeImage() else { exit(1) }

let outUrl = CFURLCreateWithFileSystemPath(nil, outputPath as CFString, .cfurlposixPathStyle, false)!
guard let dest = CGImageDestinationCreateWithURL(outUrl, "public.png" as CFString, 1, nil) else { exit(1) }
CGImageDestinationAddImage(dest, outCgImage, nil)
CGImageDestinationFinalize(dest)
print("Successfully wrote transparent icon to \(outputPath)")
