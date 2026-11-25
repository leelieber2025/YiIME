# Run from an elevated 64-bit PowerShell window.
$ErrorActionPreference = 'Stop'

function Test-PathUnderDirectory([string]$CandidatePath, [string]$DirectoryPath) {
    if ([string]::IsNullOrWhiteSpace($CandidatePath) -or [string]::IsNullOrWhiteSpace($DirectoryPath)) {
        return $false
    }
    $cand = [IO.Path]::GetFullPath($CandidatePath)
    $dir = [IO.Path]::GetFullPath($DirectoryPath).TrimEnd('\', '/') + [IO.Path]::DirectorySeparatorChar
    return $cand.StartsWith($dir, [StringComparison]::OrdinalIgnoreCase)
}

function Copy-VerifiedFile([string]$Source, [string]$Destination) {
    $expectedHash = (Get-FileHash -LiteralPath $Source -Algorithm SHA256).Hash
    if (Test-Path -LiteralPath $Destination) {
        $existingHash = (Get-FileHash -LiteralPath $Destination -Algorithm SHA256).Hash
        if ($existingHash -eq $expectedHash) {
            return
        }
    }
    Copy-Item -LiteralPath $Source -Destination $Destination -Force -ErrorAction Stop
    $actualHash = (Get-FileHash -LiteralPath $Destination -Algorithm SHA256).Hash
    if ($actualHash -ne $expectedHash) {
        throw "文件校验失败：$Destination"
    }
}

function Invoke-DllRegistration(
    [string]$Dll,
    [string]$Dll32,
    [string]$OldDll,
    [string]$OldDll32,
    [string]$Destination,
    [scriptblock]$RunRegsvr64,
    [scriptblock]$RunRegsvr32,
    [scriptblock]$SetCurrentDir,
    [scriptblock]$RestoreCurrentDir
) {
    $unregisteredOld64 = $false
    $unregisteredOld32 = $false
    $registeredNew64 = $false
    $registeredNew32 = $false
    try {
        if ($OldDll -and (Test-Path -LiteralPath $OldDll) -and $OldDll -ne $Dll) {
            $code = & $RunRegsvr64 @('/u', '/s', "`"$OldDll`"")
            if ($code -ne 0) { throw "旧版输入法注销失败：$code" }
            $unregisteredOld64 = $true
        }
        if ($OldDll32 -and (Test-Path -LiteralPath $OldDll32) -and $OldDll32 -ne $Dll32) {
            $code = & $RunRegsvr32 @('/u', '/s', "`"$OldDll32`"")
            if ($code -ne 0) { throw "旧版 32 位输入法注销失败：$code" }
            $unregisteredOld32 = $true
        }
        $code = & $RunRegsvr64 @('/s', "`"$Dll`"")
        if ($code -ne 0) { throw "新版输入法注册失败：$code" }
        $registeredNew64 = $true

        $code = & $RunRegsvr32 @('/s', "`"$Dll32`"")
        if ($code -ne 0) { throw "32 位输入法注册失败：$code" }
        $registeredNew32 = $true

        & $SetCurrentDir $Destination
    } catch {
        try { & $RestoreCurrentDir } catch {}
        if ($registeredNew32 -and $Dll32 -ne $OldDll32) {
            try { [void](& $RunRegsvr32 @('/u', '/s', "`"$Dll32`"")) } catch {}
        }
        if ($registeredNew64 -and $Dll -ne $OldDll) {
            try { [void](& $RunRegsvr64 @('/u', '/s', "`"$Dll`"")) } catch {}
        }
        if ($unregisteredOld32 -and $OldDll32 -and (Test-Path -LiteralPath $OldDll32)) {
            try { [void](& $RunRegsvr32 @('/s', "`"$OldDll32`"")) } catch {}
        }
        if ($unregisteredOld64 -and $OldDll -and (Test-Path -LiteralPath $OldDll)) {
            try { [void](& $RunRegsvr64 @('/s', "`"$OldDll`"")) } catch {}
        }
        throw
    }
}

if ($env:YIIME_PS_TEST -eq '1') { return }

$principal = New-Object Security.Principal.WindowsPrincipal([Security.Principal.WindowsIdentity]::GetCurrent())
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw '请以管理员身份运行 PowerShell，再执行此脚本。'
}
$source = Split-Path -Parent $MyInvocation.MyCommand.Path
$base = Join-Path $env:ProgramFiles 'YiIME'
$regsvr32 = Join-Path $env:WINDIR 'System32\regsvr32.exe'
# 32-bit regsvr32: registers the x86 DLL under WOW6432Node for 32-bit applications.
$regsvr32x86 = Join-Path $env:WINDIR 'SysWOW64\regsvr32.exe'
function Invoke-Regsvr32([string[]]$RegArgs, [string]$Tool = $regsvr32) {
    $proc = Start-Process -FilePath $Tool -ArgumentList $RegArgs -Wait -PassThru -WindowStyle Hidden
    return $proc.ExitCode
}
$clsid = '{A72091C2-7459-4C47-85F4-4626AE42A6C8}'
$registryPath = "Registry::HKEY_CLASSES_ROOT\CLSID\$clsid\InprocServer32"
$registryPath32 = "Registry::HKEY_CLASSES_ROOT\WOW6432Node\CLSID\$clsid\InprocServer32"
$oldDll = if (Test-Path $registryPath) { (Get-Item $registryPath).GetValue('') } else { $null }
$oldDll32 = if (Test-Path $registryPath32) { (Get-Item $registryPath32).GetValue('') } else { $null }
$stateKey = 'HKLM:\Software\YiIME'
$oldCurrentDirProp = if (Test-Path $stateKey) { Get-ItemProperty -Path $stateKey -Name 'CurrentDir' -ErrorAction SilentlyContinue } else { $null }
$hadCurrentDir = $null -ne $oldCurrentDirProp
$oldCurrentDir = if ($hadCurrentDir) { $oldCurrentDirProp.CurrentDir } else { $null }

$sourceDll = Join-Path $source 'yiime_tsf.dll'
$sourceDll32 = Join-Path $source 'yiime_tsf_x86.dll'
$sourceExe = Join-Path $source 'YiIME.exe'
if (-not (Test-Path $sourceDll) -or -not (Test-Path $sourceDll32) -or -not (Test-Path $sourceExe)) {
    throw '安装包缺少 DLL 或 EXE。'
}
$dllBytes = [IO.File]::ReadAllBytes($sourceDll) + [IO.File]::ReadAllBytes($sourceDll32)
$dllHash = [BitConverter]::ToString([Security.Cryptography.SHA256]::Create().ComputeHash($dllBytes)).Replace('-', '')
$version = $dllHash.Substring(0, 12) + '-' +
    (Get-FileHash $sourceExe -Algorithm SHA256).Hash.Substring(0, 12)
$destination = Join-Path $base $version
New-Item -ItemType Directory -Force -Path $destination | Out-Null
$dll = Join-Path $destination 'yiime_tsf.dll'
$dll32 = Join-Path $destination 'yiime_tsf_x86.dll'
$exe = Join-Path $destination 'YiIME.exe'
Copy-VerifiedFile $sourceDll $dll
Copy-VerifiedFile $sourceDll32 $dll32
Copy-VerifiedFile $sourceExe $exe
# The large lexicon (derived from rime-frost, GPL-3.0) is a separate file read by the EXE at
# runtime; every install overwrites it.
$sourceWords = Join-Path $source 'data\frost\YiIME-words.txt'
if (Test-Path $sourceWords) { Copy-Item $sourceWords (Join-Path $destination 'YiIME-words.txt') -Force -ErrorAction SilentlyContinue }

# Stage all verified binaries before touching the current registration. Loaded DLLs remain in
# their old version directory. On any registration or CurrentDir write failure, roll back both
# architectures and CurrentDir.
Invoke-DllRegistration `
    -Dll $dll `
    -Dll32 $dll32 `
    -OldDll $oldDll `
    -OldDll32 $oldDll32 `
    -Destination $destination `
    -RunRegsvr64 { param($RegArgs) Invoke-Regsvr32 $RegArgs $regsvr32 } `
    -RunRegsvr32 { param($RegArgs) Invoke-Regsvr32 $RegArgs $regsvr32x86 } `
    -SetCurrentDir {
        param($dest)
        New-Item -Path $stateKey -Force | Out-Null
        Set-ItemProperty -Path $stateKey -Name 'CurrentDir' -Value $dest -ErrorAction Stop
    } `
    -RestoreCurrentDir {
        if ($hadCurrentDir) {
            New-Item -Path $stateKey -Force | Out-Null
            Set-ItemProperty -Path $stateKey -Name 'CurrentDir' -Value $oldCurrentDir
        } elseif (Test-Path $stateKey) {
            Remove-ItemProperty -Path $stateKey -Name 'CurrentDir' -ErrorAction SilentlyContinue
        }
    }

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
Remove-Item (Join-Path $env:APPDATA "YiIME\ime-list-added"), (Join-Path $env:APPDATA "InputTranslation\ime-list-added") -Force -ErrorAction SilentlyContinue
# Trays of this version and of 0.1.x (InputTranslation-Portable.exe under Program Files\InputTranslation).
$legacyBase = Join-Path $env:ProgramFiles 'InputTranslation'
Get-CimInstance Win32_Process -Filter "Name='YiIME.exe' OR Name='InputTranslation-Portable.exe'" -ErrorAction SilentlyContinue |
    Where-Object {
        $_.CommandLine -notmatch '--tsf-hwnd' -and (
            (Test-PathUnderDirectory $_.ExecutablePath $base) -or
            (Test-PathUnderDirectory $_.ExecutablePath $legacyBase)
        )
    } |
    ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
try {
    Start-Process -FilePath (Join-Path $env:WINDIR 'explorer.exe') -ArgumentList "`"$exe`""
    Write-Host '已在托盘启动「YiIME 译」。'
} catch {
    Write-Warning "未能自动启动托盘程序：$_"
}
Write-Host '安装完成。请在 Windows 输入法切换器中选择「YiIME 译（五笔）」。'
Write-Host '若旧版文件仍被应用占用，注销并重新登录后可删除旧版目录。'
