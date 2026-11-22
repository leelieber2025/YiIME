# Authenticode-signs Windows binaries with signtool (Windows SDK). Opt-in: without a certificate
# this prints a notice and exits 0.
#
#   pwsh windows/sign.ps1 <file>...
#
# Same environment variables as windows/sign.sh: WINDOWS_SIGN_PFX or WINDOWS_SIGN_PFX_BASE64,
# WINDOWS_SIGN_PFX_PASSWORD, WINDOWS_SIGN_TIMESTAMP_URL, WINDOWS_SIGN_DESCRIPTION. SIGNTOOL may
# point to signtool.exe (default: newest one in the Windows 10/11 SDK, or signtool on PATH).
param([Parameter(Mandatory = $true, ValueFromRemainingArguments = $true)][string[]]$Files)
$ErrorActionPreference = 'Stop'
$pfx = $env:WINDOWS_SIGN_PFX
$temporary = $null
if (-not $pfx -and $env:WINDOWS_SIGN_PFX_BASE64) {
    $temporary = [System.IO.Path]::GetTempFileName()
    [System.IO.File]::WriteAllBytes($temporary, [Convert]::FromBase64String($env:WINDOWS_SIGN_PFX_BASE64))
    $pfx = $temporary
}
if (-not $pfx) {
    Write-Host "未配置代码签名证书（WINDOWS_SIGN_PFX / WINDOWS_SIGN_PFX_BASE64），跳过签名：$Files"
    exit 0
}
try {
    $signtool = $env:SIGNTOOL
    if (-not $signtool) {
        $signtool = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin\*\x64\signtool.exe" -ErrorAction SilentlyContinue |
            Sort-Object FullName -Descending | Select-Object -First 1 -ExpandProperty FullName
    }
    if (-not $signtool) { $signtool = 'signtool.exe' }
    $timestamp = if ($env:WINDOWS_SIGN_TIMESTAMP_URL) { $env:WINDOWS_SIGN_TIMESTAMP_URL } else { 'http://timestamp.digicert.com' }
    $description = if ($env:WINDOWS_SIGN_DESCRIPTION) { $env:WINDOWS_SIGN_DESCRIPTION } else { 'YiIME 译' }
    foreach ($file in $Files) {
        $arguments = @('sign', '/fd', 'SHA256', '/f', $pfx, '/d', $description,
                       '/du', 'https://github.com/leelieber2025/YiIME')
        if ($env:WINDOWS_SIGN_PFX_PASSWORD) { $arguments += @('/p', $env:WINDOWS_SIGN_PFX_PASSWORD) }
        if ($timestamp -ne 'none') { $arguments += @('/tr', $timestamp, '/td', 'SHA256') }
        & $signtool @arguments $file
        if ($LASTEXITCODE -ne 0) { throw "signtool 签名失败：$file" }
        & $signtool verify /pa $file
        if ($LASTEXITCODE -ne 0) { throw "签名校验失败：$file" }
        Write-Host "已签名：$file"
    }
} finally {
    if ($temporary) { Remove-Item $temporary -Force -ErrorAction SilentlyContinue }
}
