import Carbon
import Cocoa
import InputMethodKit

// `YiIME --register`: register, enable and select this input method, then exit.
if CommandLine.arguments.contains("--register") {
    let url = Bundle.main.bundleURL as CFURL
    print("register: \(TISRegisterInputSource(url))")
    let filter = [kTISPropertyBundleID as String: Bundle.main.bundleIdentifier ?? ""] as CFDictionary
    let list = TISCreateInputSourceList(filter, true)?.takeRetainedValue() as? [TISInputSource] ?? []
    for source in list {
        print("enable: \(TISEnableInputSource(source)) select: \(TISSelectInputSource(source))")
    }
    exit(list.isEmpty ? 1 : 0)
}

// Input method process entry: register the IMK service, and the candidate window is shared by all
// input controllers.
let connection = Bundle.main.infoDictionary?["InputMethodConnectionName"] as? String ?? "com.yiime.inputmethod.YiIME_Connection"
guard let server = IMKServer(name: connection, bundleIdentifier: Bundle.main.bundleIdentifier) else {
    NSLog("YiIME: IMKServer 启动失败")
    exit(1)
}
candidatesWindow = IMKCandidates(server: server, panelType: kIMKSingleRowSteppingCandidatePanel)
DispatchQueue.global(qos: .userInitiated).async { yiime_preload() }
NSLog("YiIME: IMKServer 已启动（%@）", connection)
NSApplication.shared.run()
