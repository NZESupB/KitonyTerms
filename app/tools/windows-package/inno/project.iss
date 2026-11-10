; 沿用旧KitonyTerms管理员安装及默认32位登记视图，载荷为x64。
[Setup]
AppName=KitonyTerms
AppId=KitonyTerms
AppVersion={{VERSION}}
PrivilegesRequired=admin
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=
CloseApplications=no
RestartApplications=no
DefaultDirName={autopf}\KitonyTerms
DefaultGroupName=KitonyTerms
UsePreviousAppDir=yes
UninstallDisplayName=KitonyTerms
UninstallDisplayIcon={app}\kitonyterms.exe
OutputDir=.
OutputBaseFilename={{BASENAME}}-setup
Compression=lzma2
SolidCompression=yes

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Files]
Source: "payload\kitonyterms.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "payload\kitonyterms-installation.txt"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\KitonyTerms"; Filename: "{app}\kitonyterms.exe"
Name: "{autodesktop}\KitonyTerms"; Filename: "{app}\kitonyterms.exe"

[Run]
Filename: "{app}\kitonyterms.exe"; Description: "Launch KitonyTerms"; Flags: nowait postinstall skipifsilent
