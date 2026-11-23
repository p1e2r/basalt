// Print the Minecraft window's bounds in points: x y w h (for cropping recordings to the window only).
import Foundation
import CoreGraphics
// MC_WINDOW_PID=<pid> restricts the match to one process (several Minecraft windows may be open)
let pidFilter = ProcessInfo.processInfo.environment["MC_WINDOW_PID"].flatMap { Int($0) }
let list = CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID) as? [[String: Any]] ?? []
guard let w = list.first(where: { ($0[kCGWindowName as String] as? String ?? "").hasPrefix("Minecraft") && (pidFilter == nil || ($0[kCGWindowOwnerPID as String] as? Int) == pidFilter) }),
      let b = w[kCGWindowBounds as String] as? [String: Any] else { print("none"); exit(1) }
print(Int(b["X"] as! Double), Int(b["Y"] as! Double), Int(b["Width"] as! Double), Int(b["Height"] as! Double))
