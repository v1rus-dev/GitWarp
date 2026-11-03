; Inno Setup script for the GitWarp installer. Built by packaging/package.sh,
; which passes these defines on the ISCC command line:
;   AppVersion, NumericVersion, SourceDir, OutputDir, OutputBaseFilename, RepoRoot

#ifndef AppVersion
  #error AppVersion is not defined (run packaging/package.sh)
#endif

[Setup]
; Fixed identifier of the application. Upgrades and the uninstaller find an
; existing install through it, so it must never change.
AppId={{F168AEA1-4763-4CBB-AD00-9BAEECACEEAE}
AppName=GitWarp
AppVersion={#AppVersion}
AppVerName=GitWarp {#AppVersion}
AppPublisher=v1rus-dev
AppPublisherURL=https://github.com/v1rus-dev/GitWarp
VersionInfoVersion={#NumericVersion}
; Per-user install without admin rights by default; the dialog lets the user
; switch to an all-users install.
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
DefaultDirName={autopf}\GitWarp
DisableProgramGroupPage=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
WizardStyle=modern
Compression=lzma2/max
SolidCompression=yes
SetupIconFile={#RepoRoot}\crates\app\assets\app-icon\gitwarp.ico
UninstallDisplayIcon={app}\gitwarp.exe
; Close a running GitWarp when upgrading.
CloseApplications=yes
OutputDir={#OutputDir}
OutputBaseFilename={#OutputBaseFilename}

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#SourceDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{autoprograms}\GitWarp"; Filename: "{app}\gitwarp.exe"
Name: "{autodesktop}\GitWarp"; Filename: "{app}\gitwarp.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\gitwarp.exe"; Description: "{cm:LaunchProgram,GitWarp}"; Flags: nowait postinstall skipifsilent
