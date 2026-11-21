#!/usr/bin/env bash
# CI helper: add YiIME in System Settings > Keyboard > Input Sources (macOS asks the user to approve
# third-party input methods there). Dumps the UI tree after each step for diagnosis.
out="$1"
id=com.apple.systempreferences
step() { sleep 2; /tmp/ax dump $id > "$out/ax-$1.txt" 2>&1; screencapture -x "$out/ax-$1.png"; }
open 'x-apple.systempreferences:com.apple.Keyboard-Settings.extension'
sleep 5
step 1-keyboard
/tmp/ax press $id "Edit…" 0
step 2-input-sources
/tmp/ax press $id "Add" 0 || /tmp/ax press $id "add" 0 || /tmp/ax press $id "+" 0
step 3-add-dialog
/tmp/ax focus $id "Use keyword to filter input sources"
/tmp/keys type "yiime"
step 4-filter
/tmp/ax row $id "YiIME"
step 5-yiime
/tmp/ax press $id "Add" 1 || /tmp/ax press $id "Add" 0
step 6-added
/tmp/ax press $id "Done" 0
step 7-done
