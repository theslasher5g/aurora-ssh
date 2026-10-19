; Inno Setup script for Aurora (Windows x64).
; Build: iscc /DAppVersion=0.1.0 /DSourceDir=..\..\dist\Aurora packaging\windows\aurora.iss

#ifndef AppVersion
  #define AppVersion "0.1.0"
#endif
#ifndef SourceDir
  #define SourceDir "..\..\dist\Aurora"
#endif

[Setup]
AppId={{6E4B7A12-3F7C-4C0E-9B8E-A0A7C9D1F2E1}
AppName=Aurora
AppVersion={#AppVersion}
AppVerName=Aurora {#AppVersion}
AppPublisher=Aurora
DefaultDirName={autopf}\Aurora
DefaultGroupName=Aurora
DisableProgramGroupPage=yes
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir=..\..\dist
OutputBaseFilename=Aurora-{#AppVersion}-windows-x64-setup
SetupIconFile=..\..\assets\icon.ico
UninstallDisplayIcon={app}\Aurora.exe
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
; In-app updates run the installer with /SILENT /CLOSEAPPLICATIONS.
CloseApplications=yes
RestartApplications=no

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"
Name: "german"; MessagesFile: "compiler:Languages\German.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#SourceDir}\Aurora.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\aurora-rdp.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\aurora-web.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\SECURITY.md"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\Aurora"; Filename: "{app}\Aurora.exe"
Name: "{autodesktop}\Aurora"; Filename: "{app}\Aurora.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\Aurora.exe"; Description: "{cm:LaunchProgram,Aurora}"; Flags: nowait postinstall skipifsilent
; Silent (in-app) update: start the new version again.
Filename: "{app}\Aurora.exe"; Flags: nowait skipifnotsilent

[Code]
{ Web interface entries need the Microsoft Edge WebView2 runtime. It is part of Windows 11
  and of current Windows 10, so this is only a hint, never a blocker. }
function WebView2Installed(): Boolean;
var
  Version: String;
begin
  Result :=
    (RegQueryStringValue(HKLM, 'SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}', 'pv', Version) and (Version <> '') and (Version <> '0.0.0.0')) or
    (RegQueryStringValue(HKCU, 'Software\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}', 'pv', Version) and (Version <> '') and (Version <> '0.0.0.0'));
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if (CurStep = ssPostInstall) and (not WizardSilent) and (not WebView2Installed()) then
    MsgBox('Aurora opens web interfaces (firewalls, NAS, Proxmox ...) with the Microsoft Edge WebView2 runtime, which was not found on this PC.' + #13#10 + #13#10 +
           'Everything else works without it. To use web entries, install the "Evergreen Runtime" from:' + #13#10 +
           'https://developer.microsoft.com/microsoft-edge/webview2/', mbInformation, MB_OK);
end;
