; YiIME Windows installer (Inno Setup 6).
;
; Same layout as Install-YiIME.ps1: Program Files\YiIME\<version dir>\ (always: an upgrade from 0.1.x
; or 0.2.0-rc.1 that sits in Program Files\InputTranslation moves to Program Files\YiIME, and the
; old directories are deleted once no application has their DLL loaded), where
; the version dir is the first 12 hex digits of the DLL and EXE SHA-256 (so reinstalling the same
; build is a no-op). The IME DLL is loaded into every process with a text box and cannot be
; overwritten while locked, so each version gets its own directory and the new DLL is registered
; (CLSID InprocServer32 points to it); old DLLs keep serving until their applications restart and
; find the new EXE via HKLM\Software\YiIME\CurrentDir.
;
; Install: ① copy files; ③ regsvr32 right after the last file, still inside the install phase: on
; error restore the old registration and abort, so Setup rolls back and reports failure instead of
; "completed"; ② grant ALL APPLICATION PACKAGES read+execute (UWP/AppContainer apps need it to load
; the DLL); ④ add the IME to
; the original user's Chinese input list; ⑤ Start menu (settings, history, guide, uninstall); ⑥
; launch the tray as the original user. Uninstall reverses this: stop our processes, unregister,
; remove from the input list, delete files. %APPDATA%\YiIME and HKCU\Software\YiIME (settings,
; keys, history, learned data; migrated from the 0.1.x InputTranslation names by the EXE) are kept.
;
; Build with windows/build-installer.sh (after windows/build.sh), which passes /DAppVersion=…
; /DVersionDir=<DLL hash 12>-<EXE hash 12>.

#ifndef AppVersion
  #error 请用 windows/build-installer.sh 构建（需要 /DAppVersion=...）
#endif
; VERSIONINFO must be numeric: a release candidate (0.2.0-rc.1) carries 0.2.0.
#ifndef VersionNumber
  #define VersionNumber AppVersion
#endif
#ifndef VersionDir
  #error 请用 windows/build-installer.sh 构建（需要 /DVersionDir=...）
#endif
#define AppName "YiIME 译"
#define Repo "..\.."
#define Exe "YiIME.exe"
#define Dll "yiime_tsf.dll"
; 32-bit DLL for 32-bit applications, registered with the 32-bit regsvr32 (qingjian.iss does the
; same with qingjian_tsf-<version>-x86.dll).
#define Dll32 "yiime_tsf_x86.dll"
; File names of 0.1.x, still present in old version directories.
#define LegacyExe "InputTranslation-Portable.exe"
#define LegacyDll "input_translation_tsf.dll"

[Setup]
AppId={{6E0C2B7A-3F1D-4E59-9A84-1C7B5D2E8F43}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher=YiIME
VersionInfoVersion={#VersionNumber}
VersionInfoDescription={#AppName} 安装程序
DefaultDirName={autopf}\YiIME
DisableDirPage=yes
; Do not reuse the 0.1.x directory (Program Files\InputTranslation) remembered for this AppId.
UsePreviousAppDir=no
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
ArchitecturesAllowed=x64
ArchitecturesInstallIn64BitMode=x64
MinVersion=10.0
PrivilegesRequired=admin
; Do not let Restart Manager close every application that has the IME DLL loaded (every application
; with a text box).
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
UninstallLogging=yes
; Optional code signing on Windows: ISCC /DSignTool=yiime /Syiime="signtool.exe sign /fd SHA256 ...
; $f" signs the setup and its uninstaller (see SIGNING.md). Without it the installer is built
; unsigned and windows/build-installer.sh may sign it afterwards.
#ifdef SignTool
SignTool={#SignTool}
SignedUninstaller=yes
#endif

[Languages]
Name: "chs"; MessagesFile: "compiler:Languages\ChineseSimplified.isl"

[Messages]
FinishedRestartLabel=旧版输入法仍被正在运行的应用加载。新旧版本同时运行可能导致输入法异常或崩溃，需要重启电脑才能完成升级。%n%n现在重启吗？
FinishedRestartMessage=旧版输入法仍被正在运行的应用加载。新旧版本同时运行可能导致输入法异常或崩溃，需要重启电脑才能完成升级。%n%n现在重启吗？
FinishedLabel=安装完成。在任务栏输入法切换器（Win+空格）中选择「YiIME 译（五笔）」即可使用。%n%n安装前已打开的应用要重新打开后才会用上新版；若输入法列表里没有它，注销并重新登录一次。

[Dirs]
Name: "{app}\{#VersionDir}"

[Files]
; Files in an existing version directory are identical (the name is their hash) and may be loaded,
; so never overwrite them.
Source: "{#Repo}\dist\{#Dll}"; DestDir: "{app}\{#VersionDir}"; Flags: onlyifdoesntexist uninsrestartdelete
Source: "{#Repo}\dist\{#Dll32}"; DestDir: "{app}\{#VersionDir}"; Flags: onlyifdoesntexist uninsrestartdelete
Source: "{#Repo}\dist\{#Exe}"; DestDir: "{app}\{#VersionDir}"; Flags: onlyifdoesntexist
; Large lexicon (derived from rime-frost, GPL-3.0), read from the EXE directory at runtime.
Source: "{#Repo}\data\frost\YiIME-words.txt"; DestDir: "{app}\{#VersionDir}"; Flags: ignoreversion
Source: "{#Repo}\windows\installer\Add-YiIMETip.ps1"; DestDir: "{app}\{#VersionDir}"; Flags: ignoreversion
Source: "{#Repo}\windows\TSF-使用说明.txt"; DestDir: "{app}\{#VersionDir}"; Flags: ignoreversion
Source: "{#Repo}\SECURITY.md"; DestDir: "{app}\{#VersionDir}"; Flags: ignoreversion
Source: "{#Repo}\dist\BUILDINFO.txt"; DestDir: "{app}\{#VersionDir}"; Flags: ignoreversion
; Licenses and attribution (same set as the ZIP).
Source: "{#Repo}\data\README.md"; DestDir: "{app}\{#VersionDir}\data"; Flags: ignoreversion
Source: "{#Repo}\data\wubi\LICENSE"; DestDir: "{app}\{#VersionDir}\data\wubi"; Flags: ignoreversion
Source: "{#Repo}\data\wubi\AUTHORS"; DestDir: "{app}\{#VersionDir}\data\wubi"; Flags: ignoreversion
Source: "{#Repo}\data\pinyin\LICENSE"; DestDir: "{app}\{#VersionDir}\data\pinyin"; Flags: ignoreversion
Source: "{#Repo}\data\pinyin\AUTHORS"; DestDir: "{app}\{#VersionDir}\data\pinyin"; Flags: ignoreversion
Source: "{#Repo}\data\wubi_extra\LICENSE-rime-wubi86-jidian"; DestDir: "{app}\{#VersionDir}\data\wubi_extra"; Flags: ignoreversion
Source: "{#Repo}\data\wubi_extra\LICENSE-chinese-xinhua"; DestDir: "{app}\{#VersionDir}\data\wubi_extra"; Flags: ignoreversion
Source: "{#Repo}\data\frost\LICENSE"; DestDir: "{app}\{#VersionDir}\data\frost"; Flags: ignoreversion
; Keep this entry last: its AfterInstall registers the DLL once every file is in place. An
; exception there is a fatal install error, so Setup rolls back and shows that it did not complete.
Source: "{#Repo}\data\opencc\LICENSE"; DestDir: "{app}\{#VersionDir}\data\opencc"; Flags: ignoreversion; AfterInstall: RegisterTip

[Registry]
; Current version directory, for an old DLL (still loaded somewhere) that cannot find the EXE next
; to it.
Root: HKLM; Subkey: "Software\YiIME"; ValueType: string; ValueName: "CurrentDir"; ValueData: "{app}\{#VersionDir}"; Flags: uninsdeletekey

[Icons]
Name: "{group}\YiIME 译 设置"; Filename: "{app}\{#VersionDir}\{#Exe}"; Parameters: "--settings"; WorkingDir: "{app}\{#VersionDir}"; Comment: "翻译 API、模式与快捷键设置"
Name: "{group}\YiIME 译 输入记忆"; Filename: "{app}\{#VersionDir}\{#Exe}"; Parameters: "--history"; WorkingDir: "{app}\{#VersionDir}"; Comment: "查看翻译过的中文与译文"
Name: "{group}\YiIME 译 使用说明"; Filename: "{app}\{#VersionDir}\TSF-使用说明.txt"
Name: "{group}\卸载 YiIME 译"; Filename: "{uninstallexe}"

[InstallDelete]
; Shortcut left by the script install (Install-YiIME.ps1) in the runner's Start
; Menu\Programs.
Type: files; Name: "{userprograms}\YiIME 设置.lnk"
Type: files; Name: "{userprograms}\中英输入 设置.lnk"

[Run]
; ② *S-1-15-2-1 = ALL APPLICATION PACKAGES (by SID, regardless of system language).
Filename: "{sys}\icacls.exe"; Parameters: """{app}"" /grant *S-1-15-2-1:(OI)(CI)RX /T /C /Q"; Flags: runhidden waituntilterminated; StatusMsg: "正在设置应用容器访问权限…"
; ④ The input list is per user: add it for the user who started the installer, not the elevated
; admin account.
Filename: "{sys}\WindowsPowerShell\v1.0\powershell.exe"; Parameters: "-NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File ""{app}\{#VersionDir}\Add-YiIMETip.ps1"" -Add"; Flags: runasoriginaluser runhidden waituntilterminated; StatusMsg: "正在加入当前用户的输入法列表…"
; ⑥ The tray runs as the original user (and re-checks the input list with InstallLayoutOrTip on
; start).
Filename: "{app}\{#VersionDir}\{#Exe}"; WorkingDir: "{app}\{#VersionDir}"; Description: "立即启动 YiIME 译（托盘）"; Flags: runasoriginaluser nowait postinstall

[UninstallRun]
; Stop all EXEs (tray and input boxes) in this install directory, unregister, then remove the IME
; from the uninstalling user's input list.
Filename: "{sys}\WindowsPowerShell\v1.0\powershell.exe"; Parameters: "-NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File ""{app}\{#VersionDir}\Add-YiIMETip.ps1"" -StopUnder ""{app}"""; Flags: runhidden waituntilterminated; RunOnceId: "StopYiIME"
; Unregistering is done in [Code] (UnregisterTip) so a failure is reported instead of ignored.
Filename: "{sys}\WindowsPowerShell\v1.0\powershell.exe"; Parameters: "-NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File ""{app}\{#VersionDir}\Add-YiIMETip.ps1"" -Remove"; Flags: runhidden waituntilterminated; RunOnceId: "RemoveTip"

[UninstallDelete]
; Version directories from earlier versions (DLLs still in use are deleted after a reboot).
Type: filesandordirs; Name: "{app}"

[Code]
const
  ClsidKey = 'CLSID\{A72091C2-7459-4C47-85F4-4626AE42A6C8}\InprocServer32';

var
  PreviousDll: String;
  PreviousDll32: String;

function CurrentDll: String;
begin
  Result := ExpandConstant('{app}\{#VersionDir}\{#Dll}');
end;

function CurrentDll32: String;
begin
  Result := ExpandConstant('{app}\{#VersionDir}\{#Dll32}');
end;

function RunRegsvr32(const Params: String): Integer;
begin
  if not Exec(ExpandConstant('{sys}\regsvr32.exe'), Params, '', SW_HIDE, ewWaitUntilTerminated, Result) then
    Result := -1;
end;

{ 32 位 regsvr32（SysWOW64）：InprocServer32 写进 WOW6432Node，供 32 位应用加载；TIP 配置两边共用。 }
function RunRegsvr32x86(const Params: String): Integer;
begin
  if not Exec(ExpandConstant('{syswow64}\regsvr32.exe'), Params, '', SW_HIDE, ewWaitUntilTerminated, Result) then
    Result := -1;
end;

{ 安装前只结束托盘（命令行不含 --tsf-hwnd 的本安装目录下的 EXE）；正在编辑的浮动框不打断。
  辅助脚本还没装上（首次安装）时用源包里的同一段逻辑：交给 PowerShell 内联执行。 }
procedure StopOldTray;
var
  ResultCode: Integer;
  Script: String;
begin
  Script := 'Get-CimInstance Win32_Process -Filter \"Name=''{#Exe}'' OR Name=''{#LegacyExe}''\" -ErrorAction SilentlyContinue | ' +
    'Where-Object { $_.CommandLine -notmatch ''--tsf-hwnd'' -and $_.ExecutablePath -like ''' + ExpandConstant('{app}') + '\*'' } | ' +
    'ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }';
  Exec(ExpandConstant('{sys}\WindowsPowerShell\v1.0\powershell.exe'),
    '-NoProfile -ExecutionPolicy Bypass -Command "' + Script + '"', '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  PreviousDll := '';
  RegQueryStringValue(HKCR, ClsidKey, '', PreviousDll);
  PreviousDll32 := '';
  RegQueryStringValue(HKCR32, ClsidKey, '', PreviousDll32);
  StopOldTray;
  Result := '';
end;

function LogHint: String;
begin
  Result := ExpandConstant('{log}');
  if Result = '' then
    Result := '（未生成日志；可用 /LOG="路径" 重新运行）';
end;

{ ③ 注册新 DLL（最后一个文件的 AfterInstall，仍在安装阶段）。失败时先把注册恢复到原来的 DLL，再抛出
  异常：Inno Setup 把它当作安装失败，回滚本次复制的文件并显示“安装未完成”，不会再显示“安装完成”，
  静默安装的退出码也不为 0。 }
procedure RegisterTip;
var
  Code: Integer;
begin
  Code := RunRegsvr32('/s "' + CurrentDll + '"');
  if Code = 0 then
  begin
    Code := RunRegsvr32x86('/s "' + CurrentDll32 + '"');
    if Code <> 0 then
    begin
      Log('32 位 regsvr32 注册失败，退出码 ' + IntToStr(Code) + '：' + CurrentDll32);
      if (PreviousDll32 <> '') and (CompareText(PreviousDll32, CurrentDll32) <> 0) and FileExists(PreviousDll32) then
        RunRegsvr32x86('/s "' + PreviousDll32 + '"');
    end;
  end;
  if Code <> 0 then
  begin
    Log('regsvr32 注册失败，退出码 ' + IntToStr(Code) + '：' + CurrentDll);
    if (PreviousDll <> '') and (CompareText(PreviousDll, CurrentDll) <> 0) and FileExists(PreviousDll) then
    begin
      if RunRegsvr32('/s "' + PreviousDll + '"') = 0 then
        Log('已恢复原来的注册 ' + PreviousDll)
      else
        Log('恢复原来的注册失败 ' + PreviousDll);
    end;
    RaiseException('YiIME 输入法注册失败（regsvr32 退出码 ' + IntToStr(Code) + '）。' + #13#10 +
      '安装已取消，本次复制的文件会被回滚；原来安装的版本（如有）保持不变。' + #13#10 +
      '安装日志：' + LogHint);
  end;
  Log('已注册 ' + CurrentDll + ' 和 ' + CurrentDll32);
end;

var
  OldDllInUse: Boolean;

{ 删掉 Base 下以前的版本目录：DLL 删得掉（没有应用在用）才删整个目录。删不掉说明仍被正在运行的应用加载：
  安排重启后删除，并要求重启（NeedRestart），避免新旧版本 DLL 混用。 }
procedure DeleteStaleVersionDirs(const Base: String);
var
  Dir, Dll: String;
  Found: TFindRec;
  Busy: Boolean;
begin
  if not DirExists(Base) then
    exit;
  if FindFirst(Base + '\*', Found) then
  begin
    try
      repeat
        if ((Found.Attributes and FILE_ATTRIBUTE_DIRECTORY) <> 0) and (Found.Name <> '.') and (Found.Name <> '..')
          and ((CompareText(Base, ExpandConstant('{app}')) <> 0) or (CompareText(Found.Name, '{#VersionDir}') <> 0)) then
        begin
          Dir := Base + '\' + Found.Name;
          Busy := False;
          { 0.1.x 目录里的 DLL 叫 {#LegacyDll}：同样先删 DLL，删不掉说明仍被加载。 }
          Dll := Dir + '\{#Dll}';
          if FileExists(Dll) and not DeleteFile(Dll) then
          begin
            Busy := True;
            try
              RestartReplace(Dll, '');
            except
              Log('无法安排重启后删除 ' + Dll);
            end;
          end;
          Dll := Dir + '\{#Dll32}';
          if FileExists(Dll) and not DeleteFile(Dll) then
          begin
            Busy := True;
            try
              RestartReplace(Dll, '');
            except
              Log('无法安排重启后删除 ' + Dll);
            end;
          end;
          Dll := Dir + '\{#LegacyDll}';
          if FileExists(Dll) and not DeleteFile(Dll) then
          begin
            Busy := True;
            try
              RestartReplace(Dll, '');
            except
              Log('无法安排重启后删除 ' + Dll);
            end;
          end;
          if Busy then
          begin
            OldDllInUse := True;
            Log('旧版本 DLL 仍被占用，重启后删除；保留 ' + Dir);
          end
          else if DelTree(Dir, True, True, True) then
            Log('已删除旧版本目录 ' + Dir)
          else
            Log('旧版本目录未能完全删除 ' + Dir);
        end;
      until not FindNext(Found);
    finally
      FindClose(Found);
    end;
  end;
end;

{ 0.1.x 与 0.2.0-rc.1 装在 Program Files\InputTranslation：版本目录按上面的规则删除，旧卸载程序一并删掉
  （卸载信息已由本次安装接管），目录空了就删除。 }
procedure DeleteLegacyInstallDir;
var
  Legacy: String;
begin
  Legacy := ExpandConstant('{autopf}\InputTranslation');
  if (CompareText(Legacy, ExpandConstant('{app}')) = 0) or not DirExists(Legacy) then
    exit;
  DeleteStaleVersionDirs(Legacy);
  DelTree(Legacy + '\unins*.*', False, True, False);
  if RemoveDir(Legacy) then
    Log('已删除旧安装目录 ' + Legacy)
  else
    Log('旧安装目录暂时保留（仍有文件在用）' + Legacy);
end;

function NeedRestart: Boolean;
begin
  Result := OldDllInUse;
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  { 注册在安装阶段完成（RegisterTip）；走到这里说明已注册成功。 }
  if CurStep = ssPostInstall then
  begin
    DeleteStaleVersionDirs(ExpandConstant('{app}'));
    DeleteLegacyInstallDir;
  end;
end;

{ 卸载：在删除任何文件之前注销输入法。注销失败时不静默继续（否则注册表仍指向即将删除的 DLL，
  输入法列表里留下用不了的条目）：说明退出码和日志位置，由用户选择继续或取消；静默卸载默认取消。
  DLL 已经不在时没有可注销的东西，直接继续。 }
procedure UnregisterTip;
var
  Dll, Dll32: String;
  Code: Integer;
begin
  { 32 位注册只删 WOW6432Node 下的 CLSID（TIP 配置由 64 位那次删）；失败只记日志。 }
  Dll32 := ExpandConstant('{app}\{#VersionDir}\{#Dll32}');
  if FileExists(Dll32) then
  begin
    Code := RunRegsvr32x86('/u /s "' + Dll32 + '"');
    if Code = 0 then
      Log('已注销 ' + Dll32)
    else
      Log('32 位 regsvr32 /u 失败，退出码 ' + IntToStr(Code) + '：' + Dll32);
  end;
  Dll := ExpandConstant('{app}\{#VersionDir}\{#Dll}');
  if not FileExists(Dll) then
  begin
    Log('DLL 不存在，跳过注销：' + Dll);
    exit;
  end;
  Code := RunRegsvr32('/u /s "' + Dll + '"');
  if Code = 0 then
  begin
    Log('已注销 ' + Dll);
    exit;
  end;
  Log('regsvr32 /u 失败，退出码 ' + IntToStr(Code) + '：' + Dll);
  if SuppressibleMsgBox('注销 YiIME 输入法失败（regsvr32 退出码 ' + IntToStr(Code) + '）。' + #13#10#13#10 +
    '继续卸载会删除程序文件，但输入法列表里可能留下无法使用的条目。' + #13#10 +
    '选“是”继续卸载；选“否”取消卸载（不删除任何文件），可重启电脑后再试。' + #13#10 +
    '卸载日志在 %TEMP% 文件夹中。',
    mbError, MB_YESNO or MB_DEFBUTTON2, IDNO) <> IDYES then
    RaiseException('卸载已取消：输入法注销失败（regsvr32 退出码 ' + IntToStr(Code) + '）。');
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usUninstall then
    UnregisterTip;
end;
