# YiIME installer helper (ASCII only on purpose).
#   -Add        add the input method to the current user's language list (run as the original user)
#   -Remove     remove it from the current user's language list
#   -StopUnder  stop every InputTranslation-Portable.exe whose path is under the given directory
param([switch]$Add, [switch]$Remove, [string]$StopUnder)
$ErrorActionPreference = 'Stop'
$tip = '0804:{A72091C2-7459-4C47-85F4-4626AE42A6C8}{54A39D73-0625-4560-B878-94660908C69D}'
$marker = Join-Path $env:APPDATA 'InputTranslation\ime-list-added'
try {
    if ($StopUnder) {
        Get-Process -Name 'InputTranslation-Portable' -ErrorAction SilentlyContinue |
            Where-Object { $_.Path -and $_.Path.StartsWith($StopUnder, [StringComparison]::OrdinalIgnoreCase) } |
            Stop-Process -Force -ErrorAction SilentlyContinue
    }
    if ($Add -or $Remove) {
        $languages = Get-WinUserLanguageList
    }
    if ($Add) {
        $chinese = $languages | Where-Object { $_.LanguageTag -in @('zh-Hans-CN', 'zh-CN') } | Select-Object -First 1
        if (-not $chinese) {
            $languages.Add('zh-Hans-CN')
            $chinese = $languages | Where-Object { $_.LanguageTag -eq 'zh-Hans-CN' } | Select-Object -First 1
        }
        if ($chinese.InputMethodTips -notcontains $tip) {
            $chinese.InputMethodTips.Add($tip)
            Set-WinUserLanguageList $languages -Force
        }
        # Let the tray re-run InstallLayoutOrTip once for this user as well.
        Remove-Item $marker -Force -ErrorAction SilentlyContinue
    }
    if ($Remove) {
        $changed = $false
        foreach ($language in $languages) {
            foreach ($item in @($language.InputMethodTips | Where-Object { $_ -eq $tip })) {
                [void]$language.InputMethodTips.Remove($item)
                $changed = $true
            }
        }
        if ($changed) { Set-WinUserLanguageList $languages -Force }
        Remove-Item $marker -Force -ErrorAction SilentlyContinue
    }
    exit 0
} catch {
    exit 1
}
