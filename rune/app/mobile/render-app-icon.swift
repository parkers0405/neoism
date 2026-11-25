import CoreGraphics
import Foundation
import ImageIO
import UniformTypeIdentifiers

// The desktop artwork has transparent padding. Preserve the artwork and its framing on an
// opaque background for iOS, which supplies its own mask (and rejects alpha in app icons).
let input = URL(fileURLWithPath: CommandLine.arguments[1])
let output = URL(fileURLWithPath: CommandLine.arguments[2])
guard let source = CGImageSourceCreateWithURL(input as CFURL, nil),
      let image = (0..<CGImageSourceGetCount(source))
          .compactMap({ CGImageSourceCreateImageAtIndex(source, $0, nil) })
          .max(by: { $0.width < $1.width }),
      let context = CGContext(data: nil, width: 1024, height: 1024, bitsPerComponent: 8,
                              bytesPerRow: 0, space: CGColorSpace(name: CGColorSpace.sRGB)!,
                              bitmapInfo: CGImageAlphaInfo.noneSkipLast.rawValue) else {
    fatalError("Could not decode the shipping Neoism icon")
}
let frame = CGRect(x: 0, y: 0, width: 1024, height: 1024)
context.setFillColor(gray: 0, alpha: 1)
context.fill(frame)
context.interpolationQuality = .none
context.draw(image, in: frame)
guard let converted = context.makeImage(),
      let destination = CGImageDestinationCreateWithURL(output as CFURL, UTType.png.identifier as CFString, 1, nil) else {
    fatalError("Could not create the iOS app icon")
}
CGImageDestinationAddImage(destination, converted, nil)
guard CGImageDestinationFinalize(destination) else { fatalError("Could not write the iOS app icon") }