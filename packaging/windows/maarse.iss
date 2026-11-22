; Installer for Må:rse on Windows (saved as UTF-8 with a byte order mark, so Inno Setup reads the å). One administrator approval (UAC) at install: it puts the program in
; Program Files and registers the Må:rse service (runs as SYSTEM from boot, runs the host and installs signed
; updates). The tray app (maarse.exe) runs as the signed-in user. Built by .github/workflows/release.yml:
;   iscc /DAppVersion=0.8.0 /DSourceDir=out\pkg packaging\windows\maarse.iss
; It appears in Windows Settings > Apps, from where it is uninstalled (the service is removed with it).
#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef SourceDir
  #define SourceDir "..\..\target\release"
#endif

[Setup]
AppId={{6B3F6A0E-5C9B-4B8E-9E55-3F0C4C0C7A11}
AppName=Må:rse
AppVersion={#AppVersion}
AppPublisher=Må:rse
DefaultDirName={autopf}\Maarse
DefaultGroupName=Maarse
PrivilegesRequired=admin
OutputBaseFilename=maarse-setup-x64
Compression=lzma2
SolidCompression=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
UninstallDisplayName=Må:rse
ShowLanguageDialog=auto
UninstallDisplayIcon={app}\maarse.exe
SetupIconFile=maarse.ico

; The installer follows the Windows language; English is the fallback. A language is added with one more
; [Languages] line and its lines in [CustomMessages].
[Languages]
Name: "en"; MessagesFile: "compiler:Default.isl"
Name: "da"; MessagesFile: "compiler:Languages\Danish.isl"

[CustomMessages]
en.CreateDesktopIcon=Create a shortcut on the desktop
da.CreateDesktopIcon=Opret en genvej på skrivebordet
en.ShowTrayAtSignIn=Show the Må:rse icon when I sign in to Windows
da.ShowTrayAtSignIn=Vis Må:rse-ikonet, når jeg logger ind på Windows
en.RegisteringService=Registering the Må:rse service ...
da.RegisteringService=Registrerer Må:rse-tjenesten ...
en.OpenNow=Open Må:rse now
da.OpenNow=Åbn Må:rse nu
en.DeleteDataQuestion=Also delete your Må:rse data (identity, messages, contacts, settings)?%nThis cannot be undone. Choose No to keep it for a later reinstall.
da.DeleteDataQuestion=Slet også dine Må:rse-data (identitet, beskeder, kontakter, indstillinger)?%nDet kan ikke fortrydes. Vælg Nej for at beholde dem til en senere geninstallation.

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"
Name: "autostart"; Description: "{cm:ShowTrayAtSignIn}"

[Files]
Source: "{#SourceDir}\maarse.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\maarse-host.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\maarse-service.exe"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\Maarse\Maarse"; Filename: "{app}\maarse.exe"
Name: "{autoprograms}\Maarse\Uninstall Maarse"; Filename: "{uninstallexe}"
Name: "{autodesktop}\Maarse"; Filename: "{app}\maarse.exe"; Tasks: desktopicon
Name: "{commonstartup}\Maarse"; Filename: "{app}\maarse.exe"; Parameters: "--no-browser"; Tasks: autostart

[Run]
; Registers the service (starts at boot) and starts it. Re-running it on an upgrade is harmless.
Filename: "{app}\maarse-service.exe"; Parameters: "install"; Flags: runhidden waituntilterminated; StatusMsg: "{cm:RegisteringService}"
Filename: "{app}\maarse.exe"; Description: "{cm:OpenNow}"; Flags: nowait postinstall skipifsilent

; Files the service's self-update leaves behind are not in [Files], so remove them explicitly.
[UninstallDelete]
Type: files; Name: "{app}\*.old"
Type: filesandordirs; Name: "{app}"

[Code]
// The tray app and the service may be running: end them before installing over them or removing them.
procedure StopMaarse;
var
  Code: Integer;
begin
  Exec('taskkill.exe', '/F /IM maarse.exe', '', SW_HIDE, ewWaitUntilTerminated, Code);
  Exec('net.exe', 'stop Maarse', '', SW_HIDE, ewWaitUntilTerminated, Code);
  Exec('taskkill.exe', '/F /IM maarse-host.exe', '', SW_HIDE, ewWaitUntilTerminated, Code);
  Sleep(500);
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  StopMaarse;
  Result := '';
end;

function InitializeUninstall: Boolean;
begin
  StopMaarse;
  Result := True;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  Code: Integer;
begin
  if CurUninstallStep = usUninstall then
    // Removes the service registration while its program is still there.
    Exec(ExpandConstant('{app}\maarse-service.exe'), 'uninstall', '', SW_HIDE, ewWaitUntilTerminated, Code);
  // The user's data (identity, messages, contacts, settings) lives in %ProgramData%\Maarse and is only
  // removed if the user asks for it.
  if CurUninstallStep = usPostUninstall then
    if MsgBox(CustomMessage('DeleteDataQuestion'), mbConfirmation, MB_YESNO or MB_DEFBUTTON2) = IDYES then
      DelTree(ExpandConstant('{commonappdata}\Maarse'), True, True, True);
end;
