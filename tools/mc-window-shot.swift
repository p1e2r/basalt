// Capture only the Minecraft client window (never the rest of the screen).
// usage: swift tools/mc-window-shot.swift out.png
import Foundation
import CoreGraphics
let out = CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : "mc.png"
let list = CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID) as? [[String: Any]] ?? []
guard let w = list.first(where: { ($0[kCGWindowName as String] as? String ?? "").hasPrefix("Minecraft") }),
      let id = w[kCGWindowNumber as String] as? Int else { print("no Minecraft window"); exit(1) }
let p = Process(); p.executableURL = URL(fileURLWithPath: "/usr/sbin/screencapture")
p.arguments = ["-x", "-o", "-l\(id)", out]; try! p.run(); p.waitUntilExit()
print("captured window \(id) -> \(out)")
