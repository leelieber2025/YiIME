# Run from an elevated 64-bit PowerShell window.
param([switch]$RemoveSettings)
$ErrorActionPreference = 'Stop'

function Test-PathUnderDirectory([string]$CandidatePath, [string]$DirectoryPath) {
    if ([string]::IsNullOrWhiteSpace($CandidatePath) -or [string]::IsNullOrWhiteSpace($DirectoryPath)) {
        return $false
    }
    $cand = [IO.Path]::GetFullPath($CandidatePath)
    $dir = [IO.Path]::GetFullPath($DirectoryPath).TrimEnd('\', '/') + [IO.Path]::DirectorySeparatorChar
    return $cand.StartsWith($dir, [StringComparison]::OrdinalIgnoreCase)
}

function Invoke-DllUnregistration(
    [string]$Dll,
    [string]$Dll32,
    [scriptblock]$RunRegsvr64,
    [scriptblock]$RunRegsvr32
) {
    if ($Dll32 -and (Test-Path -LiteralPath $Dll32)) {
        $code32 = & $RunRegsvr32 @('/u', '/s', "`"$Dll32`"")
        if ($code32 -ne 0) { throw "32 位输入法注销失败：$code32" }
    }
    if ($Dll -and (Test-Path -LiteralPath $Dll)) {
        $code64 = & $RunRegsvr64 @('/u', '/s', "`"$Dll`"")
        if ($code64 -ne 0) { throw "输入法注销失败：$code64" }
    }
}

if ($env:YIIME_PS_TEST -eq '1') { return }

$principal = New-Object Security.Principal.WindowsPrincipal([Security.Principal.WindowsIdentity]::GetCurrent())
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw '请以管理员身份运行 PowerShell，再执行此脚本。'
}
$base = Join-Path $env:ProgramFiles 'YiIME'
# Install directory, settings key and folder of 0.1.x (before the rename to YiIME); removed too.
$legacyBase = Join-Path $env:ProgramFiles 'InputTranslation'
$regsvr32 = Join-Path $env:WINDIR 'System32\regsvr32.exe'
$regsvr32x86 = Join-Path $env:WINDIR 'SysWOW64\regsvr32.exe'
function Invoke-Regsvr32([string[]]$RegArgs, [string]$Tool) {
    $proc = Start-Process -FilePath $Tool -ArgumentList $RegArgs -Wait -PassThru -WindowStyle Hidden
    return $proc.ExitCode
}
$registryPath = 'Registry::HKEY_CLASSES_ROOT\CLSID\{A72091C2-7459-4C47-85F4-4626AE42A6C8}\InprocServer32'
$dll = if (Test-Path $registryPath) { (Get-Item $registryPath).GetValue('') } else { $null }
# The 32-bit registration (WOW6432Node), for 32-bit applications.
$registryPath32 = 'Registry::HKEY_CLASSES_ROOT\WOW6432Node\CLSID\{A72091C2-7459-4C47-85F4-4626AE42A6C8}\InprocServer32'
$dll32 = if (Test-Path $registryPath32) { (Get-Item $registryPath32).GetValue('') } else { $null }
Invoke-DllUnregistration `
    -Dll $dll `
    -Dll32 $dll32 `
    -RunRegsvr64 { param($RegArgs) Invoke-Regsvr32 $RegArgs $regsvr32 } `
    -RunRegsvr32 { param($RegArgs) Invoke-Regsvr32 $RegArgs $regsvr32x86 }
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
Get-Process -Name 'YiIME', 'InputTranslation-Portable' -ErrorAction SilentlyContinue |
    Where-Object {
        (Test-PathUnderDirectory $_.Path $base) -or
        (Test-PathUnderDirectory $_.Path $legacyBase)
    } |
    Stop-Process -Force -ErrorAction SilentlyContinue
Remove-Item 'HKLM:\Software\YiIME', 'HKLM:\Software\InputTranslation' -Recurse -Force -ErrorAction SilentlyContinue
$locked = @()
$found = $false
foreach ($root in @($base, $legacyBase)) {
    if (-not (Test-Path $root)) { continue }
    $found = $true
    # Delete version directories one at a time, DLL first. If the DLL is still loaded, keep the
    # whole directory rather than leave a DLL without its EXE.
    $rootLocked = @()
    foreach ($dir in Get-ChildItem $root -Directory) {
        try {
            foreach ($name in @('yiime_tsf.dll', 'yiime_tsf_x86.dll', 'input_translation_tsf.dll')) {
                $dirDll = Join-Path $dir.FullName $name
                if (Test-Path $dirDll) { Remove-Item $dirDll -Force -ErrorAction Stop }
            }
            Remove-Item $dir.FullName -Recurse -Force -ErrorAction Stop
        } catch {
            $rootLocked += $dir.FullName
        }
    }
    if ($rootLocked.Count -eq 0) { Remove-Item $root -Force -Recurse -ErrorAction SilentlyContinue }
    $locked += $rootLocked
}
if (-not $found) {
    Write-Host '输入法已注销；安装目录不存在。'
} elseif ($locked.Count -eq 0) {
    Write-Host '已注销并删除输入法。'
} else {
    Write-Warning '输入法已注销，但以下目录的 DLL 正被应用占用。请注销 Windows 后删除：'
    $locked | ForEach-Object { Write-Warning $_ }
}
$settings = Join-Path $env:APPDATA 'YiIME'
$legacySettings = Join-Path $env:APPDATA 'InputTranslation'
Remove-Item (Join-Path $settings 'ime-list-added'), (Join-Path $legacySettings 'ime-list-added') -Force -ErrorAction SilentlyContinue
if ($RemoveSettings) {
    foreach ($dir in @($settings, $legacySettings)) {
        if (Test-Path $dir) { Remove-Item $dir -Force -Recurse }
    }
    # Mode, shortcuts, punctuation/Traditional and other switches live in HKCU\Software\YiIME
    # (HKCU\Software\InputTranslation in 0.1.x).
    Remove-Item 'HKCU:\Software\YiIME', 'HKCU:\Software\InputTranslation' -Recurse -Force -ErrorAction SilentlyContinue
    Write-Host '当前用户的 API 配置、加密密钥、输入记忆、词库学习记录和输入法开关已删除。'
} elseif ((Test-Path $settings) -or (Test-Path $legacySettings)) {
    Write-Host "当前用户的 API 配置（含输入记忆）仍保留在 $settings；如需连同 HKCU\Software\YiIME 一并删除，请加 -RemoveSettings 参数。"
}
