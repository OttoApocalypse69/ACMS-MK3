; The Windows installer, built with Inno Setup 6.3 or later:
;
;     iscc /DAppVersion=0.2.0 installer\windows\acms-mk3.iss
;
; after `cargo build --release -p acms-app`. It writes target\installer\acms-mk3-setup-x86_64.exe.
;
; By default it installs for the current user only, needing no administrator rights; the first
; page offers installing for everyone instead.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif

#define AppName "ACMS mk3"
#define AppExe "acms-app.exe"
#define Root "..\.."

[Setup]
; Identifies the app to Windows across versions: never change it.
AppId={{6ED011BB-3E9E-4CA6-9EB4-0E0400EC0AE8}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher=ACMS mk3
AppPublisherURL=https://github.com/OttoApocalypse69/ACMS-MK3
AppSupportURL=https://github.com/OttoApocalypse69/ACMS-MK3/issues
AppUpdatesURL=https://github.com/OttoApocalypse69/ACMS-MK3/releases
DefaultDirName={autopf}\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir={#Root}\target\installer
OutputBaseFilename=acms-mk3-setup-x86_64
SetupIconFile={#Root}\assets\icon\acms.ico
UninstallDisplayIcon={app}\{#AppExe}
UninstallDisplayName={#AppName}
WizardStyle=modern
Compression=lzma2/max
SolidCompression=yes
ChangesAssociations=yes
; A running copy would keep the program file locked.
CloseApplications=yes

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked
Name: "associate"; Description: "Open .acms files with {#AppName}"; GroupDescription: "Files:"

[Files]
Source: "{#Root}\target\release\{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Root}\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Root}\LICENSE-MIT"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Root}\LICENSE-APACHE"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#AppExe}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; Tasks: desktopicon

[Registry]
; The kind of file the app opens, with its icon and how to open it. HKA is the current user's
; or everyone's part of the registry, following how the app is installed; the uninstaller
; takes the entries away again.
Root: HKA; Subkey: "Software\Classes\ACMS.Sprite"; ValueType: string; ValueName: ""; ValueData: "ACMS sprite"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\ACMS.Sprite\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: "{app}\{#AppExe},0"
Root: HKA; Subkey: "Software\Classes\ACMS.Sprite\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExe}"" ""%1"""
; .acms files open in the app if the user wants that, and it is offered for them either way.
Root: HKA; Subkey: "Software\Classes\.acms"; ValueType: string; ValueName: ""; ValueData: "ACMS.Sprite"; Flags: uninsdeletevalue; Tasks: associate
Root: HKA; Subkey: "Software\Classes\.acms\OpenWithProgids"; ValueType: string; ValueName: "ACMS.Sprite"; ValueData: ""; Flags: uninsdeletevalue
; "Open with" lists the app for Aseprite files too, without taking them over.
Root: HKA; Subkey: "Software\Classes\Applications\{#AppExe}"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\Applications\{#AppExe}\SupportedTypes"; ValueType: string; ValueName: ".acms"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\{#AppExe}\SupportedTypes"; ValueType: string; ValueName: ".aseprite"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\{#AppExe}\SupportedTypes"; ValueType: string; ValueName: ".ase"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\{#AppExe}\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExe}"" ""%1"""
Root: HKA; Subkey: "Software\Classes\.aseprite\OpenWithProgids"; ValueType: string; ValueName: "ACMS.Sprite"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.ase\OpenWithProgids"; ValueType: string; ValueName: "ACMS.Sprite"; ValueData: ""; Flags: uninsdeletevalue

[Run]
Filename: "{app}\{#AppExe}"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: nowait postinstall skipifsilent
