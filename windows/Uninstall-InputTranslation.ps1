# Run from an elevated 64-bit PowerShell window.
param([switch]$RemoveSettings)
$ErrorActionPreference = 'Stop'
$principal = New-Object Security.Principal.WindowsPrincipal([Security.Principal.WindowsIdentity]::GetCurrent())
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw '请以管理员身份运行 PowerShell，再执行此脚本。'
}
$base = Join-Path $env:ProgramFiles 'InputTranslation'
$registryPath = 'Registry::HKEY_CLASSES_ROOT\CLSID\{A72091C2-7459-4C47-85F4-4626AE42A6C8}\InprocServer32'
$dll = if (Test-Path $registryPath) { (Get-Item $registryPath).GetValue('') } else { $null }
if ($dll -and (Test-Path $dll)) {
    $proc = Start-Process -FilePath (Join-Path $env:WINDIR 'System32\regsvr32.exe') -ArgumentList @('/u', '/s', "`"$dll`"") -Wait -PassThru -WindowStyle Hidden
    if ($proc.ExitCode -ne 0) { throw "输入法注销失败：$($proc.ExitCode)" }
}
# Remove the IME from the current user's input list and delete the Start menu shortcut.
$tip = '0804:{A72091C2-7459-4C47-85F4-4626AE42A6C8}{54A39D73-0625-4560-B878-94660908C69D}'
try {
    $languages = Get-WinUserLanguageList
    $changed = $false
    foreach ($language in $languages) {
        $found = @($language.InputMethodTips | Where-Object { $_ -eq $tip })
        foreach ($item in $found) { [void]$language.InputMethodTips.Remove($item); $changed = $true }
    }
    if ($changed) {
        Set-WinUserLanguageList $languages -Force
        Write-Host '已从当前用户的输入法列表中移除。'
    }
} catch {
    Write-Warning "未能从输入法列表中移除：$_"
}
foreach ($name in @('YiIME 设置.lnk', '中英输入 设置.lnk')) {
    $shortcut = Join-Path ([Environment]::GetFolderPath('Programs')) $name
    if (Test-Path $shortcut) { Remove-Item $shortcut -Force }
}
# Stop our own editor/tray processes (only EXEs in the install directory) so files can be deleted.
Get-Process -Name 'InputTranslation-Portable' -ErrorAction SilentlyContinue |
    Where-Object { $_.Path -and $_.Path.StartsWith($base, [StringComparison]::OrdinalIgnoreCase) } |
    Stop-Process -Force -ErrorAction SilentlyContinue
Remove-Item 'HKLM:\Software\InputTranslation' -Recurse -Force -ErrorAction SilentlyContinue
if (Test-Path $base) {
    # Delete version directories one at a time, DLL first. If the DLL is still loaded, keep the
    # whole directory rather than leave a DLL without its EXE.
    $locked = @()
    foreach ($dir in Get-ChildItem $base -Directory) {
        try {
            $dirDll = Join-Path $dir.FullName 'input_translation_tsf.dll'
            if (Test-Path $dirDll) { Remove-Item $dirDll -Force -ErrorAction Stop }
            Remove-Item $dir.FullName -Recurse -Force -ErrorAction Stop
        } catch {
            $locked += $dir.FullName
        }
    }
    if ($locked.Count -eq 0) {
        Remove-Item $base -Force -Recurse -ErrorAction SilentlyContinue
        Write-Host '已注销并删除输入法。'
    } else {
        Write-Warning '输入法已注销，但以下目录的 DLL 正被应用占用。请注销 Windows 后删除：'
        $locked | ForEach-Object { Write-Warning $_ }
    }
} else {
    Write-Host '输入法已注销；安装目录不存在。'
}
$settings = Join-Path $env:APPDATA 'InputTranslation'
Remove-Item (Join-Path $settings 'ime-list-added') -Force -ErrorAction SilentlyContinue
if ($RemoveSettings) {
    if (Test-Path $settings) { Remove-Item $settings -Force -Recurse }
    # Mode, shortcuts, punctuation/Traditional and other switches live in
    # HKCU\Software\InputTranslation.
    Remove-Item 'HKCU:\Software\InputTranslation' -Recurse -Force -ErrorAction SilentlyContinue
    Write-Host '当前用户的 API 配置、加密密钥、输入记忆、词库学习记录和输入法开关已删除。'
} elseif (Test-Path $settings) {
    Write-Host "当前用户的 API 配置（含输入记忆）仍保留在 $settings；如需连同 HKCU\Software\InputTranslation 一并删除，请加 -RemoveSettings 参数。"
}
