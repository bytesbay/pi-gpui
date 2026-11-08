import Foundation
import CoreGraphics
// Usage: winwait <pid> <timeout-seconds>. Prints the milliseconds until a normal-size
// window of the process (or a child process) is on screen. Used by bench.sh.
let args = CommandLine.arguments
let root = Int32(args[1])!
let timeout = Double(args[2])!
let start = Date()
func descendants(_ pid: Int32) -> Set<Int32> {
    let p = Process(); p.executableURL = URL(fileURLWithPath: "/bin/ps"); p.arguments = ["-axo", "pid=,ppid="]
    let pipe = Pipe(); p.standardOutput = pipe; try? p.run(); p.waitUntilExit()
    let out = String(data: pipe.fileHandleForReading.readDataToEndOfFile(), encoding: .utf8) ?? ""
    var children: [Int32: [Int32]] = [:]
    for line in out.split(separator: "\n") {
        let f = line.split(separator: " ").compactMap { Int32($0) }
        if f.count == 2 { children[f[1], default: []].append(f[0]) }
    }
    var set: Set<Int32> = [pid]; var stack = [pid]
    while let x = stack.popLast() { for c in children[x] ?? [] where !set.contains(c) { set.insert(c); stack.append(c) } }
    return set
}
var tick = 0
var pids: Set<Int32> = [root]
while Date().timeIntervalSince(start) < timeout {
    if tick % 10 == 0 { pids = descendants(root) }
    tick += 1
    if let list = CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID) as? [[String: Any]] {
        for w in list {
            guard let pid = w[kCGWindowOwnerPID as String] as? Int32, pids.contains(pid),
                  (w[kCGWindowLayer as String] as? Int) == 0,
                  let b = w[kCGWindowBounds as String] as? [String: Any],
                  let width = b["Width"] as? Double, let height = b["Height"] as? Double,
                  width > 300, height > 300 else { continue }
            print(Int(Date().timeIntervalSince(start) * 1000)); exit(0)
        }
    }
    usleep(10_000)
}
print("timeout"); exit(1)
