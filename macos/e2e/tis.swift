// CI helper: register, enable or select input sources, or list them. Usage: tis register <app path>
// | tis select <input source ID> | tis list | tis current
import Carbon
import Foundation

func sources(_ id: String? = nil) -> [TISInputSource] {
    var filter: [String: Any] = [:]
    if let id { filter[kTISPropertyInputSourceID as String] = id }
    let list = TISCreateInputSourceList(filter as CFDictionary, true)?.takeRetainedValue() as? [TISInputSource]
    return list ?? []
}

func property(_ source: TISInputSource, _ key: CFString) -> String {
    guard let raw = TISGetInputSourceProperty(source, key) else { return "" }
    let value = Unmanaged<AnyObject>.fromOpaque(raw).takeUnretainedValue()
    return "\(value)"
}

func enabled(_ source: TISInputSource) -> Bool {
    guard let raw = TISGetInputSourceProperty(source, kTISPropertyInputSourceIsEnabled) else { return false }
    return (Unmanaged<AnyObject>.fromOpaque(raw).takeUnretainedValue() as? NSNumber)?.boolValue == true
}

let args = CommandLine.arguments
switch args.count > 1 ? args[1] : "" {
case "register":
    let status = TISRegisterInputSource(URL(fileURLWithPath: args[2]) as CFURL)
    print("TISRegisterInputSource: \(status)")
    exit(status == noErr ? 0 : 1)
case "select":
    let found = sources(args[2])
    guard let source = found.first else { print("没有找到 \(args[2])"); exit(1) }
    let enableStatus = TISEnableInputSource(source)
    let selectStatus = TISSelectInputSource(source)
    let isEnabled = sources(args[2]).first.map(enabled) ?? false
    let current = TISCopyCurrentKeyboardInputSource().takeRetainedValue()
    let selected = property(current, kTISPropertyInputSourceID) == args[2]
    print("enumerated=true enabled=\(isEnabled) enableStatus=\(enableStatus) selected=\(selected) selectStatus=\(selectStatus) current=\(property(current, kTISPropertyInputSourceID)) visibleInSettings=unknown")
    exit(enableStatus == noErr && selectStatus == noErr && isEnabled && selected ? 0 : 1)
case "current":
    let source = TISCopyCurrentKeyboardInputSource().takeRetainedValue()
    print(property(source, kTISPropertyInputSourceID))
case "list":
    for source in sources() where property(source, kTISPropertyInputSourceID).contains(args.count > 2 ? args[2] : "") {
        print(property(source, kTISPropertyInputSourceID), property(source, kTISPropertyInputSourceType), "enabled=\(property(source, kTISPropertyInputSourceIsEnabled)) enableCapable=\(property(source, kTISPropertyInputSourceIsEnableCapable)) selectCapable=\(property(source, kTISPropertyInputSourceIsSelectCapable))")
    }
default:
    print("用法：tis register <app> | select <id> | current | list [过滤]")
    exit(2)
}
