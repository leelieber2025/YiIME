import Cocoa
import InputMethodKit
import Security

var candidatesWindow: IMKCandidates?

/// Longest Shift hold that counts as a tap (same as Windows).
private let shiftTapMax: TimeInterval = 0.6

private func takeString(_ pointer: UnsafeMutablePointer<CChar>?) -> String {
    guard let pointer else { return "" }
    defer { yiime_string_free(pointer) }
    return String(cString: pointer)
}

/// LLM settings: URL and model in UserDefaults (llmUrl, llmModel of com.yiime.inputmethod.YiIME),
/// key in the keychain (generic password, service YiIME, account llm). If unset, only free Google
/// Translate is used.
private func llmSettings() -> (String, String, String) {
    let defaults = UserDefaults(suiteName: "com.yiime.inputmethod.YiIME") ?? .standard
    let url = defaults.string(forKey: "llmUrl") ?? ""
    let model = defaults.string(forKey: "llmModel") ?? ""
    var item: CFTypeRef?
    let query: [String: Any] = [
        kSecClass as String: kSecClassGenericPassword,
        kSecAttrService as String: "YiIME",
        kSecAttrAccount as String: "llm",
        kSecReturnData as String: true,
        kSecMatchLimit as String: kSecMatchLimitOne,
    ]
    var key = ""
    if SecItemCopyMatching(query as CFDictionary, &item) == errSecSuccess, let data = item as? Data {
        key = String(decoding: data, as: UTF8.self)
    }
    return (url, model, key)
}

@objc(YiIMEInputController)
class YiIMEInputController: IMKInputController {
    private let ime = yiime_new()
    private var candidates: [String] = []
    private var shiftDown: Date?
    private var translating = false
    /// Chinese text being translated: committed as is if the session ends before the result.
    private var pendingSource = ""
    /// Client of the current session (the one activateServer was called for).
    private weak var currentClient: AnyObject?

    deinit { yiime_free(ime) }

    override func recognizedEvents(_ sender: Any!) -> Int {
        Int(NSEvent.EventTypeMask([.keyDown, .flagsChanged]).rawValue)
    }

    override func handle(_ event: NSEvent!, client sender: Any!) -> Bool {
        guard let event, let client = sender as? IMKTextInput else { return false }
        if event.type == .flagsChanged {
            return handleFlags(event, client: client)
        }
        guard event.type == .keyDown else { return false }
        shiftDown = nil
        if translating { return true }
        let flags = event.modifierFlags.intersection([.command, .control, .option])
        if !flags.isEmpty { return false }
        let active = yiime_active(ime)
        switch event.keyCode {
        case 36, 76: // Return / Enter
            guard active else { return false }
            // Enter while composing: the code goes into the draft as letters (as in Microsoft
            // Pinyin), then everything is committed.
            yiime_commit_code(ime)
            if event.modifierFlags.contains(.shift) {
                commit(takeString(yiime_take_text(ime)), client: client)
            } else {
                translate(client: client)
            }
            return true
        case 51: // Backspace
            return send("\u{8}", client: client)
        case 53: // Esc
            return send("\u{1b}", client: client)
        case 48, 123, 124, 125, 126: // Tab, arrow keys
            return active
        default:
            break
        }
        guard let chars = event.characters, let scalar = chars.unicodeScalars.first, chars.unicodeScalars.count == 1 else {
            return active
        }
        return send(Character(scalar), client: client)
    }

    private func handleFlags(_ event: NSEvent, client: IMKTextInput) -> Bool {
        let shift = event.modifierFlags.contains(.shift)
        let others = !event.modifierFlags.intersection([.command, .control, .option]).isEmpty
        if shift && !others && (event.keyCode == 56 || event.keyCode == 60) {
            if shiftDown == nil { shiftDown = Date() }
            return false
        }
        if !shift, let start = shiftDown, Date().timeIntervalSince(start) <= shiftTapMax, yiime_active(ime) {
            shiftDown = nil
            yiime_shift_tap(ime)
            refresh(client: client)
            return true
        }
        shiftDown = nil
        return false
    }

    private func send(_ ch: Character, client: IMKTextInput) -> Bool {
        guard let scalar = ch.unicodeScalars.first else { return false }
        let handled = yiime_key(ime, scalar.value)
        if handled { refresh(client: client) }
        return handled
    }

    private func state() -> [String: Any] {
        let json = takeString(yiime_state(ime))
        return (try? JSONSerialization.jsonObject(with: Data(json.utf8))) as? [String: Any] ?? [:]
    }

    /// Shows draft + code as marked text at the cursor, candidates in the candidate window.
    private func refresh(client: IMKTextInput) {
        let state = self.state()
        let draft = state["draft"] as? String ?? ""
        let code = state["code"] as? String ?? ""
        let english = state["english"] as? Bool ?? false
        // An English segment is marked with "[英]" at the end (display only, never committed).
        let marked = draft + code + (english ? "[英]" : "")
        let attributes: [NSAttributedString.Key: Any] = [.underlineStyle: NSUnderlineStyle.single.rawValue]
        client.setMarkedText(NSAttributedString(string: marked, attributes: attributes),
                             selectionRange: NSRange(location: ((draft + code) as NSString).length, length: 0),
                             replacementRange: NSRange(location: NSNotFound, length: NSNotFound))
        let items = state["candidates"] as? [[String: Any]] ?? []
        candidates = items.enumerated().map { index, item in
            "\(index + 1).\(item["text"] as? String ?? "")\(item["hint"] as? String ?? "")"
        }
        if candidates.isEmpty {
            candidatesWindow?.hide()
        } else {
            candidatesWindow?.update()
            candidatesWindow?.show(kIMKLocateCandidatesBelowHint)
        }
    }

    override func candidates(_ sender: Any!) -> [Any]! { candidates }

    private func commit(_ text: String, client: IMKTextInput) {
        candidatesWindow?.hide()
        client.insertText(text, replacementRange: NSRange(location: NSNotFound, length: NSNotFound))
    }

    private func translate(client: IMKTextInput) {
        let source = takeString(yiime_take_text(ime))
        guard !source.isEmpty else { return }
        candidatesWindow?.hide()
        // The result may arrive after the user switched apps or turned the input method off; it
        // is committed only if this request is still current (see yiime_finish_request).
        let request = yiime_begin_request(ime)
        let requestClient = client as AnyObject
        pendingSource = source
        translating = true
        client.setMarkedText(source + " …", selectionRange: NSRange(location: 0, length: 0),
                             replacementRange: NSRange(location: NSNotFound, length: NSNotFound))
        let (url, model, key) = llmSettings()
        DispatchQueue.global(qos: .userInitiated).async {
            let json = takeString(yiime_translate(source, url, model, key))
            let value = (try? JSONSerialization.jsonObject(with: Data(json.utf8))) as? [String: Any] ?? [:]
            let ok = value["ok"] as? Bool ?? false
            let text = ok ? (value["text"] as? String ?? source) : source
            if !ok { NSLog("YiIME: 翻译失败，上屏中文：%@", value["error"] as? String ?? "") }
            DispatchQueue.main.async {
                guard yiime_finish_request(self.ime, request),
                      self.currentClient == nil || self.currentClient === requestClient else {
                    // Logged without the text.
                    NSLog("YiIME: 翻译结果已丢弃（会话已结束或已切换应用）")
                    return
                }
                self.translating = false
                self.pendingSource = ""
                self.commit(text, client: client)
            }
        }
    }

    /// Ends a pending translation: its result will be dropped, and the Chinese text goes to
    /// `client` (the app being left) so nothing typed is lost.
    private func abandonTranslation(client: IMKTextInput?) {
        guard translating else { return }
        yiime_cancel_request(ime)
        translating = false
        let source = pendingSource
        pendingSource = ""
        if let client, !source.isEmpty {
            commit(source, client: client)
        }
    }

    override func activateServer(_ sender: Any!) {
        super.activateServer(sender)
        let client = sender.map { $0 as AnyObject }
        if let client, client !== currentClient {
            // Another client: a translation started elsewhere must not land here.
            abandonTranslation(client: currentClient as? IMKTextInput)
        }
        currentClient = client
    }

    override func commitComposition(_ sender: Any!) {
        guard let client = sender as? IMKTextInput else { return }
        if translating {
            abandonTranslation(client: client)
            return
        }
        guard yiime_active(ime) else { return }
        commit(takeString(yiime_take_text(ime)), client: client)
    }

    override func deactivateServer(_ sender: Any!) {
        candidatesWindow?.hide()
        abandonTranslation(client: sender as? IMKTextInput)
        currentClient = nil
        super.deactivateServer(sender)
    }
}
