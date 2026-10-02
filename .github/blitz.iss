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
OutputDir={#OutDir}
OutputBaseFilename=blitz-{#AppVersion}-windows-x64-setup

[Tasks]
Name: desktopicon; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#SrcDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs

[Icons]
Name: "{autoprograms}\blitz"; Filename: "{app}\blitz.exe"
Name: "{autodesktop}\blitz"; Filename: "{app}\blitz.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\blitz.exe"; Description: "{cm:LaunchProgram,blitz}"; Flags: nowait postinstall skipifsilent
