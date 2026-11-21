// CI helper: inspect and drive another app through the Accessibility API.
// Usage: ax dump|press|row|focus <bundle-id> [label] [index]
import AppKit
import ApplicationServices

func attr(_ e: AXUIElement, _ name: String) -> AnyObject? {
    var value: AnyObject?
    return AXUIElementCopyAttributeValue(e, name as CFString, &value) == .success ? value : nil
}
func text(_ e: AXUIElement, _ name: String) -> String { (attr(e, name) as? String) ?? "" }
func children(_ e: AXUIElement) -> [AXUIElement] { (attr(e, kAXChildrenAttribute) as? [AXUIElement]) ?? [] }
func labels(_ e: AXUIElement) -> [String] {
    [text(e, kAXTitleAttribute), text(e, kAXDescriptionAttribute), text(e, kAXIdentifierAttribute),
     (attr(e, kAXValueAttribute) as? String) ?? ""].filter { !$0.isEmpty }
}
func walk(_ e: AXUIElement, _ depth: Int = 0, _ visit: (AXUIElement, Int) -> Void) {
    guard depth < 40 else { return }
    visit(e, depth)
    for child in children(e) { walk(child, depth + 1, visit) }
}
let args = CommandLine.arguments
guard args.count >= 3, let app = NSRunningApplication.runningApplications(withBundleIdentifier: args[2]).first else {
    print("app not running or bad usage; trusted=\(AXIsProcessTrusted())"); exit(2)
}
let root = AXUIElementCreateApplication(app.processIdentifier)
print("trusted=\(AXIsProcessTrusted())")
switch args[1] {
case "dump":
    walk(root) { e, d in
        print(String(repeating: " ", count: d) + text(e, kAXRoleAttribute) + " " + labels(e).joined(separator: " | "))
    }
case "press":
    var found: [AXUIElement] = []
    walk(root) { e, _ in if labels(e).contains(args[3]) { found.append(e) } }
    let index = args.count > 4 ? Int(args[4]) ?? 0 : 0
    guard index < found.count else { print("not found: \(args[3]) (\(found.count))"); exit(1) }
    print("press \(args[3]) #\(index): \(AXUIElementPerformAction(found[index], kAXPressAction as CFString).rawValue)")
case "row":
    var done = false
    walk(root) { e, _ in
        guard !done, text(e, kAXRoleAttribute) == kAXRowRole as String else { return }
        var hit = false
        walk(e) { c, _ in if labels(c).contains(where: { $0.contains(args[3]) }) { hit = true } }
        if hit {
            let r = AXUIElementSetAttributeValue(e, kAXSelectedAttribute as CFString, kCFBooleanTrue)
            print("select row \(args[3]): \(r.rawValue)")
            done = true
        }
    }
    if !done { print("row not found: \(args[3])"); exit(1) }
case "focus":
    var found: AXUIElement?
    walk(root) { e, _ in if found == nil, labels(e).contains(args[3]) { found = e } }
    guard let found else { print("not found: \(args[3])"); exit(1) }
    print("focus \(args[3]): \(AXUIElementSetAttributeValue(found, kAXFocusedAttribute as CFString, kCFBooleanTrue).rawValue)")
default:
    exit(2)
}
