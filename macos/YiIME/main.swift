import Carbon
import Cocoa
import InputMethodKit

// `YiIME --register`: report each registration stage separately.
if CommandLine.arguments.contains("--register") {
    let url = Bundle.main.bundleURL as CFURL
    let registered = TISRegisterInputSource(url)
    print("registered=\(registered == noErr) status=\(registered)")
    let modeID = "com.yiime.inputmethod.YiIME.Hans"
    let filter = [kTISPropertyInputSourceID as String: modeID] as CFDictionary
    let list = TISCreateInputSourceList(filter, true)?.takeRetainedValue() as? [TISInputSource] ?? []
    print("enumerated=\(!list.isEmpty) id=\(modeID)")
    guard let source = list.first else {
        print("enabled=false selected=false visibleInSettings=unknown")
        exit(1)
    }
    let enableStatus = TISEnableInputSource(source)
    let selectStatus = TISSelectInputSource(source)
    func sourceID(_ source: TISInputSource) -> String {
        guard let raw = TISGetInputSourceProperty(source, kTISPropertyInputSourceID) else { return "" }
        return "\(Unmanaged<AnyObject>.fromOpaque(raw).takeUnretainedValue())"
    }
    let refreshed = (TISCreateInputSourceList(filter, true)?.takeRetainedValue() as? [TISInputSource])?.first ?? source
    let enabled = TISGetInputSourceProperty(refreshed, kTISPropertyInputSourceIsEnabled)
        .map { (Unmanaged<AnyObject>.fromOpaque($0).takeUnretainedValue() as? NSNumber)?.boolValue == true } ?? false
    let current = TISCopyCurrentKeyboardInputSource().takeRetainedValue()
    let selected = sourceID(current) == modeID
    print("enabled=\(enabled) status=\(enableStatus)")
    print("selected=\(selected) status=\(selectStatus) current=\(sourceID(current))")
    print("visibleInSettings=unknown")
    exit(registered == noErr && enableStatus == noErr && selectStatus == noErr && enabled && selected ? 0 : 1)
}

// Input method process: register the IMK server; one candidate window is shared by all controllers.
let connection = Bundle.main.infoDictionary?["InputMethodConnectionName"] as? String ?? "com.yiime.inputmethod.YiIME_Connection"
guard let server = IMKServer(name: connection, bundleIdentifier: Bundle.main.bundleIdentifier) else {
    NSLog("YiIME: IMKServer 启动失败")
    exit(1)
}
candidatesWindow = IMKCandidates(server: server, panelType: kIMKSingleRowSteppingCandidatePanel)
DispatchQueue.global(qos: .userInitiated).async { yiime_preload() }
NSLog("YiIME: IMKServer 已启动（%@）", connection)
NSApplication.shared.run()
