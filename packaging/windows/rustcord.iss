; Inno Setup script for the RustCord Windows installer.
;
; Built by .github/workflows/release.yml on the windows runner (Inno Setup 6
; is preinstalled there). To build it by hand on Windows:
;
;   cargo build --release
;   "C:\Program Files (x86)\Inno Setup 6\ISCC.exe" /DAppVersion=0.1.0 ^
;       /DSourceDir=..\.. /DOutputDir=..\..\dist packaging\windows\rustcord.iss
;
; Installs per user (no admin prompt) into %LOCALAPPDATA%\Programs\RustCord,
; adds a Start-menu shortcut (desktop shortcut optional) and an uninstaller
; under Settings -> Apps. Uninstalling keeps your settings / saved login.

#ifndef AppVersion
  #define AppVersion "0.1.0"
#endif
#ifndef SourceDir
  #define SourceDir "..\.."
#endif
#ifndef OutputDir
  #define OutputDir "..\..\dist"
#endif

[Setup]
; Never change AppId: Windows uses it to find the installed copy for upgrades.
AppId={{81324E76-11B4-495F-92B0-A4F0C8083D97}
AppName=RustCord
AppVersion={#AppVersion}
AppVerName=RustCord {#AppVersion}
AppPublisher=KeoLogs
AppPublisherURL=https://github.com/KeoLogs/rustcord
AppSupportURL=https://github.com/KeoLogs/rustcord/issues
AppUpdatesURL=https://github.com/KeoLogs/rustcord/releases
DefaultDirName={autopf}\RustCord
DefaultGroupName=RustCord
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
LicenseFile={#SourceDir}\LICENSE
SetupIconFile={#SourceDir}\assets\icon\rustcord.ico
UninstallDisplayIcon={app}\rustcord.exe
UninstallDisplayName=RustCord
OutputDir={#OutputDir}
OutputBaseFilename=RustCord-{#AppVersion}-windows-x86_64-setup
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
CloseApplications=yes

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#SourceDir}\target\release\rustcord.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\LICENSE"; DestDir: "{app}"; DestName: "LICENSE.txt"; Flags: ignoreversion
Source: "{#SourceDir}\README.md"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\RustCord"; Filename: "{app}\rustcord.exe"
Name: "{autodesktop}\RustCord"; Filename: "{app}\rustcord.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\rustcord.exe"; Description: "{cm:LaunchProgram,RustCord}"; Flags: nowait postinstall skipifsilent
