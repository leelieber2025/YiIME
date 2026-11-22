// CI helper: simulate keystrokes with CGEvent (through the current input method). Usage: keys type
// <ASCII text> | keys shift-tap | keys shift-return | keys return | keys save
import CoreGraphics
import Foundation

let codes: [Character: CGKeyCode] = [
    "a": 0, "s": 1, "d": 2, "f": 3, "h": 4, "g": 5, "z": 6, "x": 7, "c": 8, "v": 9, "b": 11, "q": 12,
    "w": 13, "e": 14, "r": 15, "y": 16, "t": 17, "1": 18, "2": 19, "3": 20, "4": 21, "6": 22, "5": 23,
    "9": 25, "7": 26, "8": 28, "0": 29, "o": 31, "u": 32, "i": 34, "p": 35, "l": 37, "j": 38, "k": 40,
    "n": 45, "m": 46, " ": 49,
]
let source = CGEventSource(stateID: .hidSystemState)

func post(_ key: CGKeyCode, down: Bool, flags: CGEventFlags = []) {
    let event = CGEvent(keyboardEventSource: source, virtualKey: key, keyDown: down)!
    event.flags = flags
    event.post(tap: .cghidEventTap)
    usleep(60_000)
}

func press(_ key: CGKeyCode, flags: CGEventFlags = []) {
    post(key, down: true, flags: flags)
    post(key, down: false, flags: flags)
}

func flagsChanged(_ flags: CGEventFlags) {
    let event = CGEvent(keyboardEventSource: source, virtualKey: 56, keyDown: !flags.isEmpty)!
    event.type = .flagsChanged
    event.flags = flags
    event.post(tap: .cghidEventTap)
    usleep(80_000)
}

let args = CommandLine.arguments
switch args.count > 1 ? args[1] : "" {
case "type":
    for ch in args[2] {
        if ch.isUppercase, let key = codes[Character(ch.lowercased())] {
            flagsChanged(.maskShift)
            press(key, flags: .maskShift)
            flagsChanged([])
        } else if let key = codes[ch] {
            press(key)
        } else {
            print("不支持的字符 \(ch)")
            exit(2)
        }
    }
case "shift-tap":
    flagsChanged(.maskShift)
    flagsChanged([])
case "shift-return":
    flagsChanged(.maskShift)
    press(36, flags: .maskShift)
    flagsChanged([])
case "return":
    press(36)
case "save":
    press(1, flags: .maskCommand)
default:
    print("用法：keys type <文本> | shift-tap | shift-return | return")
    exit(2)
}
