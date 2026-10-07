; Per-user installer for blitz. The release workflow builds it with
;   iscc /DAppVersion=<version> /DSrcDir=<staged files> /DOutDir=<output> .github\blitz.iss

[Setup]
AppId={{1DA65EB8-E92D-465A-9531-C4B72BB72597}
AppName=blitz
AppVersion={#AppVersion}
AppPublisher=blitz contributors
AppPublisherURL=https://github.com/thomasbunch/blitz
AppSupportURL=https://github.com/thomasbunch/blitz/issues
DefaultDirName={autopf}\blitz
; A folder picked by hand may be writable by every user, who could then
; replace the hook that Claude Code runs. Both defaults are not.
DisableDirPage=yes
DisableProgramGroupPage=yes
; No admin rights needed: installs under %LOCALAPPDATA%\Programs unless the
; user picks an all-users install.
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
; Windows 10 1809, the first release with ConPTY.
MinVersion=10.0.17763
CloseApplications=yes
WizardStyle=modern
Compression=lzma2
SolidCompression=yes
UninstallDisplayIcon={app}\blitz.exe
SetupIconFile=..\crates\blitz\icon\blitz.ico
OutputDir={#OutDir}
OutputBaseFilename=blitz-{#AppVersion}-windows-x64-setup

[Tasks]
Name: desktopicon; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked
Name: explorermenu; Description: "Add ""Open in blitz"" to the folder right-click menu"

[Files]
Source: "{#SrcDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs

[Registry]
; So "blitz" starts it from Win+R, and from "start blitz" in a shell.
Root: HKA; Subkey: "Software\Microsoft\Windows\CurrentVersion\App Paths\blitz.exe"; ValueType: string; ValueData: "{app}\blitz.exe"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Microsoft\Windows\CurrentVersion\App Paths\blitz.exe"; ValueType: string; ValueName: "Path"; ValueData: "{app}"
; Right-click menu entries. HKA is HKCU on a per-user install and HKLM on an
; all-users one. Windows 11 shows them under "Show more options". The second
; verb is Extended: it only shows on Shift+right-click.
; Right-click on a folder.
Root: HKA; Subkey: "Software\Classes\Directory\shell\blitz"; ValueType: string; ValueData: "Open in blitz"; Flags: uninsdeletekey; Tasks: explorermenu
Root: HKA; Subkey: "Software\Classes\Directory\shell\blitz\command"; ValueType: string; ValueData: """{app}\blitz.exe"" --cwd ""%V"""; Tasks: explorermenu
Root: HKA; Subkey: "Software\Classes\Directory\shell\blitz.window"; ValueType: string; ValueData: "Open in new blitz window"; Flags: uninsdeletekey; Tasks: explorermenu
Root: HKA; Subkey: "Software\Classes\Directory\shell\blitz.window"; ValueType: string; ValueName: "Extended"; ValueData: ""; Tasks: explorermenu
Root: HKA; Subkey: "Software\Classes\Directory\shell\blitz.window\command"; ValueType: string; ValueData: """{app}\blitz.exe"" --new-window --cwd ""%V"""; Tasks: explorermenu
; Right-click on the empty space in a folder.
Root: HKA; Subkey: "Software\Classes\Directory\Background\shell\blitz"; ValueType: string; ValueData: "Open in blitz"; Flags: uninsdeletekey; Tasks: explorermenu
Root: HKA; Subkey: "Software\Classes\Directory\Background\shell\blitz\command"; ValueType: string; ValueData: """{app}\blitz.exe"" --cwd ""%V"""; Tasks: explorermenu
Root: HKA; Subkey: "Software\Classes\Directory\Background\shell\blitz.window"; ValueType: string; ValueData: "Open in new blitz window"; Flags: uninsdeletekey; Tasks: explorermenu
Root: HKA; Subkey: "Software\Classes\Directory\Background\shell\blitz.window"; ValueType: string; ValueName: "Extended"; ValueData: ""; Tasks: explorermenu
Root: HKA; Subkey: "Software\Classes\Directory\Background\shell\blitz.window\command"; ValueType: string; ValueData: """{app}\blitz.exe"" --new-window --cwd ""%V"""; Tasks: explorermenu
; Right-click on a drive.
Root: HKA; Subkey: "Software\Classes\Drive\shell\blitz"; ValueType: string; ValueData: "Open in blitz"; Flags: uninsdeletekey; Tasks: explorermenu
Root: HKA; Subkey: "Software\Classes\Drive\shell\blitz\command"; ValueType: string; ValueData: """{app}\blitz.exe"" --cwd ""%V"""; Tasks: explorermenu
Root: HKA; Subkey: "Software\Classes\Drive\shell\blitz.window"; ValueType: string; ValueData: "Open in new blitz window"; Flags: uninsdeletekey; Tasks: explorermenu
Root: HKA; Subkey: "Software\Classes\Drive\shell\blitz.window"; ValueType: string; ValueName: "Extended"; ValueData: ""; Tasks: explorermenu
Root: HKA; Subkey: "Software\Classes\Drive\shell\blitz.window\command"; ValueType: string; ValueData: """{app}\blitz.exe"" --new-window --cwd ""%V"""; Tasks: explorermenu
; Unticking the task on a reinstall removes the entries.
Root: HKA; Subkey: "Software\Classes\Directory\shell\blitz"; Flags: deletekey dontcreatekey; Check: not WizardIsTaskSelected('explorermenu')
Root: HKA; Subkey: "Software\Classes\Directory\shell\blitz.window"; Flags: deletekey dontcreatekey; Check: not WizardIsTaskSelected('explorermenu')
Root: HKA; Subkey: "Software\Classes\Directory\Background\shell\blitz"; Flags: deletekey dontcreatekey; Check: not WizardIsTaskSelected('explorermenu')
Root: HKA; Subkey: "Software\Classes\Directory\Background\shell\blitz.window"; Flags: deletekey dontcreatekey; Check: not WizardIsTaskSelected('explorermenu')
Root: HKA; Subkey: "Software\Classes\Drive\shell\blitz"; Flags: deletekey dontcreatekey; Check: not WizardIsTaskSelected('explorermenu')
Root: HKA; Subkey: "Software\Classes\Drive\shell\blitz.window"; Flags: deletekey dontcreatekey; Check: not WizardIsTaskSelected('explorermenu')
; The name and icon blitz gives its notifications when it first shows one.
Root: HKCU; Subkey: "Software\Classes\AppUserModelId\blitz.terminal"; Flags: uninsdeletekey dontcreatekey

[Icons]
Name: "{autoprograms}\blitz"; Filename: "{app}\blitz.exe"
Name: "{autodesktop}\blitz"; Filename: "{app}\blitz.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\blitz.exe"; Description: "{cm:LaunchProgram,blitz}"; Flags: nowait postinstall skipifsilent
; blitz updates itself with /VERYSILENT /relaunch=1 and exits, so start it
; again, as the user and not elevated.
Filename: "{app}\blitz.exe"; Flags: nowait runasoriginaluser; Check: Relaunch

[Code]
var
  Installed: Boolean;

function Relaunch: Boolean;
begin
  Result := WizardSilent and (ExpandConstant('{param:relaunch|0}') = '1');
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if CurStep = ssPostInstall then
    Installed := True;
end;

// An update that failed starts the old blitz again, which shows the log.
procedure DeinitializeSetup;
var
  Code: Integer;
begin
  if Relaunch and not Installed then
    try
      ExecAsOriginalUser(ExpandConstant('{app}\blitz.exe'), '', '', SW_SHOWNORMAL, ewNoWait, Code);
    except
      // Setup stopped before it knew the folder.
    end;
end;

// Hooks pasted into Claude Code's settings name blitz-hook.exe, which is
// about to go, and Claude Code would report each of them failing.
procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  Settings: String;
  Text: AnsiString;
begin
  if (CurUninstallStep <> usUninstall) or UninstallSilent then
    Exit;
  Settings := GetEnv('CLAUDE_CONFIG_DIR');
  if Settings = '' then
    Settings := ExpandConstant('{%USERPROFILE}\.claude');
  Settings := AddBackslash(Settings) + 'settings.json';
  if LoadStringFromFile(Settings, Text) and (Pos('blitz-hook', Lowercase(String(Text))) > 0) then
    MsgBox(Settings + ' still runs blitz-hook.exe. Remove those hooks, or Claude Code will report them failing on every event.', mbInformation, MB_OK);
end;
