#!/usr/bin/env bash
# macOS end-to-end test (macOS virtual machine of GitHub Actions): Install pkg → Register and select
# the input source → Simulate typing in "Text Edit" → Take a screenshot → Use AppleScript to read
# back the text and assert.
set -uo pipefail
cd "$(dirname "$0")/../.."
version="${1:-0.1.0}"
out="${E2E_OUT:-dist/macos-e2e}"
mkdir -p "$out"
results="$out/results.txt"
: > "$results"
fail=0
check() { # check <name> <conditional command…>
    local name="$1"; shift
    if "$@"; then echo "PASS $name" | tee -a "$results"; else echo "FAIL $name" | tee -a "$results"; fail=1; fi
}
shot() { screencapture -x "$out/$1.png" 2>>"$out/screencapture.err" || echo "截图失败 $1" >> "$results"; }

sw_vers | tee "$out/system.txt"
csrutil status >> "$out/system.txt" 2>&1 || true

check "安装 pkg" sudo installer -pkg "dist/YiIME-$version-macos.pkg" -target /
app="/Library/Input Methods/YiIME.app"
check "已装到 /Library/Input Methods" test -x "$app/Contents/MacOS/YiIME"
check "Info.plist 键齐全" bash -c "for k in InputMethodConnectionName InputMethodServerControllerClass tsInputMethodCharacterRepertoireKey tsInputMethodIconFileKey TISInputSourceID CFBundleIdentifier; do /usr/libexec/PlistBuddy -c \"Print :\$k\" '$app/Contents/Info.plist' >/dev/null || exit 1; done"
check "通用二进制 arm64+x86_64" bash -c "lipo -archs '$app/Contents/MacOS/YiIME' | grep -q arm64 && lipo -archs '$app/Contents/MacOS/YiIME' | grep -q x86_64"
check "签名有效（ad-hoc）" codesign --verify "$app"

swiftc -O macos/e2e/tis.swift -o /tmp/tis && swiftc -O macos/e2e/keys.swift -o /tmp/keys && swiftc -O macos/e2e/ax.swift -o /tmp/ax
for db in "$HOME/Library/Application Support/com.apple.TCC/TCC.db" "/Library/Application Support/com.apple.TCC/TCC.db"; do
    sudo python3 macos/e2e/grant.py "$db" /tmp/keys /tmp/tis /tmp/ax /bin/bash /bin/zsh /usr/bin/osascript /usr/sbin/screencapture \
        "$(command -v python3)" com.apple.Terminal 2>&1 | tee -a "$out/tcc.txt" || true
done

check "注册输入源" /tmp/tis register "$app"
sleep 2
/tmp/tis list yiime | tee "$out/tis-list.txt"
check "输入源出现在 TIS 列表" grep -q com.yiime.inputmethod.YiIME "$out/tis-list.txt"

# Text editing opens a plain text file; to read back the text, press Cmd+S to save and then read the
# file (no AppleScript is used to avoid automated authorization pop-ups).
doc=/tmp/yiime-e2e.txt
: > "$doc"
open -a TextEdit "$doc"
sleep 4
shot 01-textedit
doc_text() { /tmp/keys save; sleep 1; cat "$doc"; }

# Baseline: First type on a US keyboard and confirm that the simulated keys can reach text editing.
/tmp/keys type "abc "
check "模拟按键可达（美式键盘打出 abc）" bash -c "/tmp/keys save; sleep 1; grep -qi '^abc' $doc"

# Enable the input source: 1) the IME registers itself; 2) TIS from a helper; 3) UI scripting in System Settings.
selected() { /tmp/tis current | grep -q com.yiime.inputmethod.YiIME; }
"$app/Contents/MacOS/YiIME" --register 2>&1 | tee "$out/select-0.txt"
sleep 2
shot 00a-after-self-register
if ! selected; then
    /tmp/tis select com.yiime.inputmethod.YiIME.Hans 2>&1 | tee "$out/select-1.txt"
    sleep 2
    shot 00b-after-tis-select
fi
if ! selected; then
    bash macos/e2e/add-input-source.sh "$out" > "$out/ui-script.txt" 2>&1
    sleep 2
    shot 00c-after-ui-script
    /tmp/tis list yiime | tee -a "$out/tis-list.txt"
    /tmp/tis select com.yiime.inputmethod.YiIME.Hans 2>&1 | tee "$out/select-2.txt"
    sleep 2
fi
killall "System Settings" 2>/dev/null || true
open -a TextEdit "$doc"
sleep 2
/tmp/tis current | tee "$out/current.txt"
check "当前输入源是 YiIME" grep -q com.yiime.inputmethod.YiIME "$out/current.txt"
shot 01b-textedit-yiime

# Wubi: q space → me; pinyin zhongguo space → Chinese; Shift and English OK; Shift+Enter to display
# Chinese.
/tmp/keys type "q "
/tmp/keys type "zhongguo"
sleep 1
shot 02-candidates-zhongguo
/tmp/keys type " "
/tmp/keys shift-tap
/tmp/keys type "OK"
sleep 1
shot 03-inline-english
/tmp/keys shift-tap
/tmp/keys shift-return
sleep 1
shot 04-committed-chinese
pgrep -lx YiIME | tee "$out/process.txt"
check "输入法进程已由系统启动" test -s "$out/process.txt"
text="$(doc_text)"
echo "上屏中文后：$text" | tee -a "$results"
check "Shift+Enter 上屏“我中国OK”" bash -c "[[ \"$text\" == *我中国OK* ]]"

# Translation: nihao Space → Hello, Enter → English (Free Google).
/tmp/keys type "nihao "
sleep 1
shot 05-before-translate
/tmp/keys return
sleep 8
shot 06-translated
text="$(doc_text)"
echo "翻译后：$text" | tee -a "$results"
check "Enter 翻译上屏英文" bash -c "[[ \"$text\" == *我中国OK* ]] && printf '%s' \"\${text#*我中国OK}\" | grep -Eiq 'hello|hi'"

log show --last 10m --predicate 'process == "YiIME" OR eventMessage CONTAINS[c] "yiime"' --style compact 2>&1 | grep -v AMFI > "$out/yiime.log" || true
cp "$doc" "$out/textedit-content.txt"
killall TextEdit 2>/dev/null || true
echo "---" >> "$results"
[ "$fail" = 0 ] && echo "全部通过" | tee -a "$results" || echo "有失败项" | tee -a "$results"
exit "$fail"
