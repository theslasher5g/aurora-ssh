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

[Languages]
Name: "german"; MessagesFile: "compiler:Languages\German.isl"
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#SourceDir}\Aurora.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\aurora-rdp.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\SECURITY.md"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\Aurora"; Filename: "{app}\Aurora.exe"
Name: "{autodesktop}\Aurora"; Filename: "{app}\Aurora.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\Aurora.exe"; Description: "{cm:LaunchProgram,Aurora}"; Flags: nowait postinstall skipifsilent
