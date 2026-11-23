// Print the Minecraft window's bounds in points: x y w h (for cropping recordings to the window only).
import Foundation
import CoreGraphics
let list = CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID) as? [[String: Any]] ?? []
guard let w = list.first(where: { ($0[kCGWindowName as String] as? String ?? "").hasPrefix("Minecraft") }),
      let b = w[kCGWindowBounds as String] as? [String: Any] else { print("none"); exit(1) }
print(Int(b["X"] as! Double), Int(b["Y"] as! Double), Int(b["Width"] as! Double), Int(b["Height"] as! Double))
