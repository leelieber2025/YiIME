; YiIME translation for Windows installer (Inno Setup 6).
;
; The layout follows Install-InputTranslation.ps1: installed to Program
; Files\InputTranslation\<version directory>\, version directory = the first 12 bits of SHA-256 of
; DLL and EXE (consistent with script installation, the same build will not be installed
; repeatedly). The input method DLL will be loaded into each process with a text box, and the file
; cannot be overwritten if it is locked, so each version is placed side by side in its own
; directory, and the new DLL is registered (the InprocServer32 of CLSID points to the new file); the
; old DLL that is still loaded by the application continues to serve until the application restarts.
; When it cannot find the EXE in its own directory, press HKLM\Software\InputTranslation\CurrentDir
; to find the new version.
;
; Installation steps: ① Copy files; ② Add ALL APPLICATION PACKAGES to the installation directory to
; read + execute (required for UWP/AppContainer applications to load DLL); ③ regsvr32 registration
; (restore the old registration and report an error if it fails); ④ Add the input method to the
; Chinese input method list as the original user; ⑤ Start menu (settings, input memory, instructions
; for use, uninstall); ⑥ Once completed, launch the tray as the original user. Uninstall reverse:
; end the program process, unregister, remove from the input method list, and then delete the file;
; %APPDATA%\InputTranslation and HKCU\Software\InputTranslation (user settings, keys, input memory,
; learning records) are retained.
;
; Structure: Version side-by-side DLL, icacls, CloseApplications=no, only end own EXE before
; installation.
;
; Usage: windows/build-installer.sh <version> (first windows/build.sh generates dist\), which passes
; in /DAppVersion=… /DVersionDir=<DLL first 12 digits>-<EXE first 12 digits>

#ifndef AppVersion
  #define AppVersion "0.1.1"
#endif
#ifndef VersionDir
  #error 请用 windows/build-installer.sh 构建（需要 /DVersionDir=...）
#endif
#define AppName "YiIME 译"
#define Repo "..\.."
#define Exe "InputTranslation-Portable.exe"
#define Dll "input_translation_tsf.dll"

[Setup]
AppId={{6E0C2B7A-3F1D-4E59-9A84-1C7B5D2E8F43}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher=YiIME
VersionInfoVersion=0.1.1.0
VersionInfoDescription={#AppName} 安装程序
DefaultDirName={autopf}\InputTranslation
DisableDirPage=yes
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
ArchitecturesAllowed=x64
ArchitecturesInstallIn64BitMode=x64
MinVersion=10.0
PrivilegesRequired=admin
; Don't let the Restart Manager close every application that has an input method DLL loaded (that's
; every application with a text box).
CloseApplications=no
RestartApplications=no
UsedUserAreasWarning=no
OutputDir={#Repo}\dist
OutputBaseFilename=YiIME-{#AppVersion}-setup
SetupIconFile={#Repo}\windows\tsf\ime.ico
UninstallDisplayIcon={app}\{#VersionDir}\{#Exe}
UninstallDisplayName={#AppName}
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
SetupLogging=yes

[Languages]
Name: "chs"; MessagesFile: "compiler:Languages\ChineseSimplified.isl"

[Messages]
FinishedLabel=安装完成。在任务栏输入法切换器（Win+空格）中选择「YiIME 译（五笔）」即可使用。%n%n安装前已打开的应用要重新打开后才会用上新版；若输入法列表里没有它，注销并重新登录一次。

[Dirs]
Name: "{app}\{#VersionDir}"

[Files]
; The contents of existing DLL/EXEs in the same version directory must be the same (the directory
; name is their hash), and they will not be overwritten if they already exist (they may be being
; loaded).
Source: "{#Repo}\dist\{#Dll}"; DestDir: "{app}\{#VersionDir}"; Flags: onlyifdoesntexist uninsrestartdelete
Source: "{#Repo}\dist\{#Exe}"; DestDir: "{app}\{#VersionDir}"; Flags: onlyifdoesntexist
; Large lexicon (rime-frost derivative, GPL-3.0): reads from the same directory when the EXE is run.
Source: "{#Repo}\data\frost\InputTranslation-words.txt"; DestDir: "{app}\{#VersionDir}"; Flags: ignoreversion
Source: "{#Repo}\windows\installer\Add-YiIMETip.ps1"; DestDir: "{app}\{#VersionDir}"; Flags: ignoreversion
Source: "{#Repo}\windows\TSF-使用说明.txt"; DestDir: "{app}\{#VersionDir}"; Flags: ignoreversion
Source: "{#Repo}\dist\BUILDINFO.txt"; DestDir: "{app}\{#VersionDir}"; Flags: ignoreversion
; License and Attribution (the same set as the ZIP package).
Source: "{#Repo}\data\README.md"; DestDir: "{app}\{#VersionDir}\data"; Flags: ignoreversion
Source: "{#Repo}\data\wubi\LICENSE"; DestDir: "{app}\{#VersionDir}\data\wubi"; Flags: ignoreversion
Source: "{#Repo}\data\wubi\AUTHORS"; DestDir: "{app}\{#VersionDir}\data\wubi"; Flags: ignoreversion
Source: "{#Repo}\data\pinyin\LICENSE"; DestDir: "{app}\{#VersionDir}\data\pinyin"; Flags: ignoreversion
Source: "{#Repo}\data\pinyin\AUTHORS"; DestDir: "{app}\{#VersionDir}\data\pinyin"; Flags: ignoreversion
Source: "{#Repo}\data\wubi_extra\LICENSE-rime-wubi86-jidian"; DestDir: "{app}\{#VersionDir}\data\wubi_extra"; Flags: ignoreversion
Source: "{#Repo}\data\wubi_extra\LICENSE-chinese-xinhua"; DestDir: "{app}\{#VersionDir}\data\wubi_extra"; Flags: ignoreversion
Source: "{#Repo}\data\frost\LICENSE"; DestDir: "{app}\{#VersionDir}\data\frost"; Flags: ignoreversion
Source: "{#Repo}\data\opencc\LICENSE"; DestDir: "{app}\{#VersionDir}\data\opencc"; Flags: ignoreversion

[Registry]
; Current version directory: Used when an old version of the DLL (still in a running application)
; cannot find the EXE in its own directory.
Root: HKLM; Subkey: "Software\InputTranslation"; ValueType: string; ValueName: "CurrentDir"; ValueData: "{app}\{#VersionDir}"; Flags: uninsdeletekey

[Icons]
Name: "{group}\YiIME 译 设置"; Filename: "{app}\{#VersionDir}\{#Exe}"; Parameters: "--settings"; WorkingDir: "{app}\{#VersionDir}"; Comment: "Translation API, mode and shortcut key settings"
Name: "{group}\YiIME 译 输入记忆"; Filename: "{app}\{#VersionDir}\{#Exe}"; Parameters: "--history"; WorkingDir: "{app}\{#VersionDir}"; Comment: "View translated Chinese and translations"
Name: "{group}\YiIME 译 使用说明"; Filename: "{app}\{#VersionDir}\TSF-Instructions for use.txt"
Name: "{group}\卸载 YiIME 译"; Filename: "{uninstallexe}"

[InstallDelete]
; The script installation (Install-InputTranslation.ps1) builds the old shortcut in the runner's
; "Start Menu\Programs".
Type: files; Name: "{userprograms}\YiIME settings.lnk"
Type: files; Name: "{userprograms}\Chinese and English input settings.lnk"

[Run]
; ② *S-1-15-2-1 = ALL APPLICATION PACKAGES (by SID, regardless of system language).
Filename: "{sys}\icacls.exe"; Parameters: """{app}"" /grant *S-1-15-2-1:(OI)(CI)RX /T /C /Q"; Flags: runhidden waituntilterminated; StatusMsg: "Configure application container permissions..."
; ④ The input method list is for each user: Join as the original user who started the installation
; program (not the upgraded administrator account).
Filename: "{sys}\WindowsPowerShell\v1.0\powershell.exe"; Parameters: "-NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File ""{app}\{#VersionDir}\Add-YiIMETip.ps1"" -Add"; Flags: runasoriginaluser runhidden waituntilterminated; StatusMsg: "Add the current user's input method list..."
; ⑥ The tray runs as the original user (it will also use InstallLayoutOrTip to ensure the input
; method list again when it starts).
Filename: "{app}\{#VersionDir}\{#Exe}"; WorkingDir: "{app}\{#VersionDir}"; Description: "Start YiIME translation immediately (tray)"; Flags: runasoriginaluser nowait postinstall

[UninstallRun]
; End all EXEs (trays and floating boxes) in this installation directory, unregister them, and then
; remove them from the input method list of the person running the uninstaller.
Filename: "{sys}\WindowsPowerShell\v1.0\powershell.exe"; Parameters: "-NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File ""{app}\{#VersionDir}\Add-YiIMETip.ps1"" -StopUnder ""{app}"""; Flags: runhidden waituntilterminated; RunOnceId: "StopYiIME"
Filename: "{sys}\regsvr32.exe"; Parameters: "/u /s ""{app}\{#VersionDir}\{#Dll}"""; Flags: runhidden waituntilterminated; RunOnceId: "UnregTip"
Filename: "{sys}\WindowsPowerShell\v1.0\powershell.exe"; Parameters: "-NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File ""{app}\{#VersionDir}\Add-YiIMETip.ps1"" -Remove"; Flags: runhidden waituntilterminated; RunOnceId: "RemoveTip"

[UninstallDelete]
; Version directories left over from previous versions (DLLs still occupied will be deleted after
; Windows restarts).
Type: filesandordirs; Name: "{app}"

[Code]
const
  ClsidKey = 'CLSID\{A72091C2-7459-4C47-85F4-4626AE42A6C8}\InprocServer32';

var
  PreviousDll: String;

function CurrentDll: String;
begin
  Result := ExpandConstant('{app}\{#VersionDir}\{#Dll}');
end;

function RunRegsvr32(const Params: String): Integer;
begin
  if not Exec(ExpandConstant('{sys}\regsvr32.exe'), Params, '', SW_HIDE, ewWaitUntilTerminated, Result) then
    Result := -1;
end;

{ 安装前只结束托盘（命令行不含 --tsf-hwnd 的本安装目录下的 EXE）；正在编辑的浮动框不打断。
  辅助脚本还没装上（首次安装）时用源包里的同一段逻辑：交给 PowerShell 内联执行。 }
procedure StopOldTray;
var
  ResultCode: Integer;
  Script: String;
begin
  Script := 'Get-CimInstance Win32_Process -Filter \"Name=''{#Exe}''\" -ErrorAction SilentlyContinue | ' +
    'Where-Object { $_.CommandLine -notmatch ''--tsf-hwnd'' -and $_.ExecutablePath -like ''' + ExpandConstant('{app}') + '\*'' } | ' +
    'ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }';
  Exec(ExpandConstant('{sys}\WindowsPowerShell\v1.0\powershell.exe'),
    '-NoProfile -ExecutionPolicy Bypass -Command "' + Script + '"', '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  PreviousDll := '';
  RegQueryStringValue(HKCR, ClsidKey, '', PreviousDll);
  StopOldTray;
  Result := '';
end;

{ ③ 注册新 DLL；失败时把注册恢复到原来的 DLL，并告诉用户（文件已复制，可重新运行安装）。 }
procedure RegisterTip;
var
  Code: Integer;
begin
  Code := RunRegsvr32('/s "' + CurrentDll + '"');
  if Code <> 0 then
  begin
    if (PreviousDll <> '') and (CompareText(PreviousDll, CurrentDll) <> 0) and FileExists(PreviousDll) then
      RunRegsvr32('/s "' + PreviousDll + '"');
    SuppressibleMsgBox('输入法注册失败（regsvr32 返回 ' + IntToStr(Code) + '）。已恢复原来的注册；请稍后重新运行安装程序。',
      mbError, MB_OK, IDOK);
  end
  else
    Log('已注册 ' + CurrentDll);
end;

{ 删掉以前的版本目录：DLL 删得掉（没有应用在用）才删整个目录；仍在用的整个保留，下次安装再试。 }
procedure DeleteStaleVersionDirs;
var
  Base, Dir: String;
  Found: TFindRec;
begin
  Base := ExpandConstant('{app}');
  if FindFirst(Base + '\*', Found) then
  begin
    try
      repeat
        if ((Found.Attributes and FILE_ATTRIBUTE_DIRECTORY) <> 0) and (Found.Name <> '.') and (Found.Name <> '..')
          and (CompareText(Found.Name, '{#VersionDir}') <> 0) then
        begin
          Dir := Base + '\' + Found.Name;
          if (not FileExists(Dir + '\{#Dll}')) or DeleteFile(Dir + '\{#Dll}') then
          begin
            if DelTree(Dir, True, True, True) then
              Log('已删除旧版本目录 ' + Dir)
            else
              Log('旧版本目录未能完全删除 ' + Dir);
          end
          else
            Log('旧版本 DLL 仍被占用，保留 ' + Dir);
        end;
      until not FindNext(Found);
    finally
      FindClose(Found);
    end;
  end;
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if CurStep = ssPostInstall then
  begin
    RegisterTip;
    DeleteStaleVersionDirs;
  end;
end;
