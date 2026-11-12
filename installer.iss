; Inno Setup script: builds VKadre-Setup-<version>.exe from dist\VKadre (file is UTF-8 with BOM)
; 1) build.bat   2) open this file in Inno Setup 6 (jrsoftware.org) and press Compile, or run:
;    "C:\Program Files (x86)\Inno Setup 6\ISCC.exe" installer.iss

#define AppName "В кадре"
#define AppId "VKadre"
#define AppVersion "0.1.0"
#define AppExe "VKadre.exe"

[Setup]
AppId={{8F3C2A51-6B7E-4D2A-9C1F-5E4B7A2D9C10}
AppName={#AppName}
AppVersion={#AppVersion}
DefaultDirName={autopf}\{#AppId}
DefaultGroupName={#AppName}
OutputDir=dist
OutputBaseFilename={#AppId}-Setup-{#AppVersion}
Compression=lzma2/max
SolidCompression=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
LicenseFile=LICENSE
UninstallDisplayIcon={app}\{#AppExe}
WizardStyle=modern

[Languages]
Name: "ru"; MessagesFile: "compiler:Languages\Russian.isl"
Name: "en"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"

[Files]
Source: "dist\VKadre\*"; DestDir: "{app}"; Flags: recursesubdirs ignoreversion

[Icons]
Name: "{group}\{#AppName}"; Filename: "{app}\{#AppExe}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; Tasks: desktopicon

[Run]
Filename: "{app}\{#AppExe}"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: nowait postinstall skipifsilent
