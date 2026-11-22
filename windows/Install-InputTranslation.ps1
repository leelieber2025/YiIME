# Run from an elevated 64-bit PowerShell window.
$ErrorActionPreference = 'Stop'
$principal = New-Object Security.Principal.WindowsPrincipal([Security.Principal.WindowsIdentity]::GetCurrent())
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw '请以管理员身份运行 PowerShell，再执行此脚本。'
}
$source = Split-Path -Parent $MyInvocation.MyCommand.Path
$base = Join-Path $env:ProgramFiles 'InputTranslation'
$regsvr32 = Join-Path $env:WINDIR 'System32\regsvr32.exe'
function Invoke-Regsvr32([string[]]$RegArgs) {
    $proc = Start-Process -FilePath $regsvr32 -ArgumentList $RegArgs -Wait -PassThru -WindowStyle Hidden
    return $proc.ExitCode
}
$clsid = '{A72091C2-7459-4C47-85F4-4626AE42A6C8}'
$registryPath = "Registry::HKEY_CLASSES_ROOT\CLSID\$clsid\InprocServer32"
$oldDll = if (Test-Path $registryPath) { (Get-Item $registryPath).GetValue('') } else { $null }
$sourceDll = Join-Path $source 'input_translation_tsf.dll'
$sourceExe = Join-Path $source 'InputTranslation-Portable.exe'
if (-not (Test-Path $sourceDll) -or -not (Test-Path $sourceExe)) {
    throw '安装包缺少 DLL 或 EXE。'
}
$version = (Get-FileHash $sourceDll -Algorithm SHA256).Hash.Substring(0, 12) + '-' +
    (Get-FileHash $sourceExe -Algorithm SHA256).Hash.Substring(0, 12)
$destination = Join-Path $base $version
New-Item -ItemType Directory -Force -Path $destination | Out-Null
$dll = Join-Path $destination 'input_translation_tsf.dll'
$exe = Join-Path $destination 'InputTranslation-Portable.exe'
if (-not (Test-Path $dll)) { Copy-Item $sourceDll $dll }
if (-not (Test-Path $exe)) { Copy-Item $sourceExe $exe }
# The large lexicon (derived from rime-frost, GPL-3.0) is a separate file read by the EXE at
# runtime; every install overwrites it.
$sourceWords = Join-Path $source 'data\frost\InputTranslation-words.txt'
if (Test-Path $sourceWords) { Copy-Item $sourceWords (Join-Path $destination 'InputTranslation-words.txt') -Force -ErrorAction SilentlyContinue }
# Stage both files before touching the current registration. Loaded DLLs remain in their old version directory.
if ($oldDll -and (Test-Path $oldDll) -and $oldDll -ne $dll) {
    $unregCode = Invoke-Regsvr32 @('/u', '/s', "`"$oldDll`"")
    if ($unregCode -ne 0) { throw "旧版输入法注销失败：$unregCode" }
}
$registrationError = Invoke-Regsvr32 @('/s', "`"$dll`"")
if ($registrationError -ne 0) {
    if ($oldDll -and (Test-Path $oldDll) -and $oldDll -ne $dll) { [void](Invoke-Regsvr32 @('/s', "`"$oldDll`"")) }
    throw "新版输入法注册失败：$registrationError"
}
# Record the current version directory, for an older DLL (still loaded by a running application)
# that cannot find the EXE next to it.
$stateKey = 'HKLM:\Software\InputTranslation'
New-Item -Path $stateKey -Force | Out-Null
Set-ItemProperty -Path $stateKey -Name 'CurrentDir' -Value $destination
# Add the IME to the current user's Chinese input list; the taskbar switcher hides IMEs not in the
# list.
$tip = '0804:{A72091C2-7459-4C47-85F4-4626AE42A6C8}{54A39D73-0625-4560-B878-94660908C69D}'
try {
    $languages = Get-WinUserLanguageList
    $chinese = $languages | Where-Object { $_.LanguageTag -in @('zh-Hans-CN', 'zh-CN') } | Select-Object -First 1
    if (-not $chinese) {
        $languages.Add('zh-Hans-CN')
        $chinese = $languages | Where-Object { $_.LanguageTag -eq 'zh-Hans-CN' } | Select-Object -First 1
    }
    if ($chinese.InputMethodTips -notcontains $tip) {
        $chinese.InputMethodTips.Add($tip)
        Set-WinUserLanguageList $languages -Force
        Write-Host "已把输入法加入当前用户（$env:USERNAME）的中文输入法列表。"
    }
} catch {
    Write-Warning "未能自动加入输入法列表：$_"
    Write-Warning '请在 设置 → 时间和语言 → 语言和区域 → 中文（简体）→ 语言选项 → 添加键盘 中选择「YiIME 译（五笔）」。'
}
# Start menu shortcut that opens the translation settings without the IME.
$programs = [Environment]::GetFolderPath('Programs')
$oldShortcut = Join-Path $programs '中英输入 设置.lnk'
if (Test-Path $oldShortcut) { Remove-Item $oldShortcut -Force -ErrorAction SilentlyContinue }
$shortcut = (New-Object -ComObject WScript.Shell).CreateShortcut((Join-Path $programs 'YiIME 设置.lnk'))
$shortcut.TargetPath = $exe
$shortcut.Arguments = '--settings'
$shortcut.WorkingDirectory = $destination
$shortcut.Description = '打开 YiIME 译的翻译 API 设置'
$shortcut.Save()
Write-Host '已创建开始菜单快捷方式「YiIME 设置」。'
# Start the new tray as the current (non-admin) user: the input list written by the elevated process
# sometimes misses the user session, so the tray re-checks it with InstallLayoutOrTip on start
# (clearing the marker forces that).
Remove-Item (Join-Path $env:APPDATA "InputTranslation\ime-list-added") -Force -ErrorAction SilentlyContinue
Get-CimInstance Win32_Process -Filter "Name='InputTranslation-Portable.exe'" -ErrorAction SilentlyContinue |
    Where-Object { $_.CommandLine -notmatch '--tsf-hwnd' -and $_.ExecutablePath -like "$base*" } |
    ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
try {
    Start-Process -FilePath (Join-Path $env:WINDIR 'explorer.exe') -ArgumentList "`"$exe`""
    Write-Host '已在托盘启动「YiIME 译」。'
} catch {
    Write-Warning "未能自动启动托盘程序：$_"
}
Write-Host '安装完成。请在 Windows 输入法切换器中选择「YiIME 译（五笔）」。'
Write-Host '若旧版文件仍被应用占用，注销并重新登录后可删除旧版目录。'
