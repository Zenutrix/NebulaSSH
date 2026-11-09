; NebulaSSH – Installer (NSIS, Modern UI 2)
; Bauen: installer\build.ps1

Unicode true
SetCompressor /SOLID lzma
ManifestDPIAware true

!define APP        "NebulaSSH"
!define PUBLISHER  "Thomas Schöpf"
!ifndef VERSION
  !define VERSION  "2.1.0"
!endif
!define EXE        "nebulassh.exe"
!define UNINST_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP}"
!define BRAND      "..\branding\out"

Name "${APP}"
Caption "${APP} ${VERSION} – Setup"
BrandingText "${APP} ${VERSION}  ·  by ${PUBLISHER}"
OutFile "..\dist\NebulaSSH-Setup-${VERSION}.exe"
InstallDir "$PROGRAMFILES64\${APP}"
InstallDirRegKey HKLM "Software\${APP}" "InstallDir"
!ifdef PREVIEW
  RequestExecutionLevel user
  OutFile "..\dist\preview-setup.exe"
!else
  RequestExecutionLevel admin
!endif

VIProductVersion "${VERSION}.0"
VIAddVersionKey /LANG=1031 "ProductName" "${APP}"
VIAddVersionKey /LANG=1031 "CompanyName" "${PUBLISHER}"
VIAddVersionKey /LANG=1031 "LegalCopyright" "© 2026 ${PUBLISHER}"
VIAddVersionKey /LANG=1031 "FileDescription" "${APP} Setup"
VIAddVersionKey /LANG=1031 "FileVersion" "${VERSION}"
VIAddVersionKey /LANG=1031 "ProductVersion" "${VERSION}"

!include "MUI2.nsh"
!include "x64.nsh"
!include "LogicLib.nsh"
!include "FileFunc.nsh"

; --- Aussehen ---
!define MUI_ICON   "${BRAND}\icon.ico"
!define MUI_UNICON "${BRAND}\icon.ico"
!define MUI_WELCOMEFINISHPAGE_BITMAP   "${BRAND}\installer-welcome.bmp"
!define MUI_UNWELCOMEFINISHPAGE_BITMAP "${BRAND}\installer-welcome.bmp"
!define MUI_HEADERIMAGE
!define MUI_HEADERIMAGE_RIGHT
!define MUI_HEADERIMAGE_BITMAP   "${BRAND}\installer-header.bmp"
!define MUI_HEADERIMAGE_UNBITMAP "${BRAND}\installer-header.bmp"
!define MUI_ABORTWARNING

!define MUI_WELCOMEPAGE_TITLE "Willkommen bei ${APP} ${VERSION}"
!define MUI_WELCOMEPAGE_TEXT "Dieser Assistent installiert ${APP} auf deinem Computer.$\r$\n$\r$\n${APP} ist ein schneller, nativer SSH-, SFTP- und Serial-Client für Windows – mit Tabs, Datei-Explorer, Editor, SSH-Key-Generator und verschlüsselter Geräteverwaltung.$\r$\n$\r$\nby ${PUBLISHER}$\r$\n$\r$\nKlicke auf Weiter, um fortzufahren."

!define MUI_FINISHPAGE_TITLE "${APP} ist bereit"
!define MUI_FINISHPAGE_TEXT "${APP} wurde erfolgreich installiert.$\r$\n$\r$\nDeine Geräte und Einstellungen werden im Benutzerprofil gespeichert (%APPDATA%\${APP}). Über Einstellungen → Daten & Backup kannst du sie jederzeit sichern oder von einem anderen PC übernehmen."
!define MUI_FINISHPAGE_RUN
!define MUI_FINISHPAGE_RUN_TEXT "${APP} jetzt starten"
!define MUI_FINISHPAGE_RUN_FUNCTION StartApp
!define MUI_FINISHPAGE_LINK "Datenordner öffnen"
!define MUI_FINISHPAGE_LINK_LOCATION "$APPDATA\${APP}"

; --- Seiten ---
!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_COMPONENTS
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_WELCOME
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_UNPAGE_FINISH

!insertmacro MUI_LANGUAGE "German"

; App ohne Administratorrechte starten (über den Explorer)
Function StartApp
  Exec '"$WINDIR\explorer.exe" "$INSTDIR\${EXE}"'
FunctionEnd

Section "${APP} (erforderlich)" SecMain
  SectionIn RO
  SetOutPath "$INSTDIR"

  ; Laufende Instanz beenden, damit die Datei ersetzt werden kann
  nsExec::Exec 'taskkill /IM ${EXE} /F'
  Sleep 300

  File "..\target\release\${EXE}"
  File "/oname=NebulaSSH.ico" "${BRAND}\icon.ico"

  WriteUninstaller "$INSTDIR\Uninstall.exe"
  WriteRegStr HKLM "Software\${APP}" "InstallDir" "$INSTDIR"

  ; Eintrag in "Apps & Features"
  WriteRegStr   HKLM "${UNINST_KEY}" "DisplayName"     "${APP}"
  WriteRegStr   HKLM "${UNINST_KEY}" "DisplayVersion"  "${VERSION}"
  WriteRegStr   HKLM "${UNINST_KEY}" "Publisher"       "${PUBLISHER}"
  WriteRegStr   HKLM "${UNINST_KEY}" "DisplayIcon"     "$INSTDIR\NebulaSSH.ico"
  WriteRegStr   HKLM "${UNINST_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr   HKLM "${UNINST_KEY}" "UninstallString" '"$INSTDIR\Uninstall.exe"'
  WriteRegStr   HKLM "${UNINST_KEY}" "QuietUninstallString" '"$INSTDIR\Uninstall.exe" /S'
  WriteRegDWORD HKLM "${UNINST_KEY}" "NoModify" 1
  WriteRegDWORD HKLM "${UNINST_KEY}" "NoRepair" 1
  SectionGetSize ${SecMain} $0
  WriteRegDWORD HKLM "${UNINST_KEY}" "EstimatedSize" $0

  ; Startmenü (für alle Benutzer)
  SetShellVarContext all
  CreateShortCut "$SMPROGRAMS\${APP}.lnk" "$INSTDIR\${EXE}" "" "$INSTDIR\NebulaSSH.ico" 0 SW_SHOWNORMAL "" "SSH-, SFTP- und Serial-Client"
SectionEnd

Section "Desktop-Symbol" SecDesktop
  SetShellVarContext all
  CreateShortCut "$DESKTOP\${APP}.lnk" "$INSTDIR\${EXE}" "" "$INSTDIR\NebulaSSH.ico" 0
SectionEnd

Function .onInit
  ${IfNot} ${RunningX64}
    MessageBox MB_ICONSTOP "${APP} benötigt ein 64-Bit-Windows."
    Abort
  ${EndIf}
  SetRegView 64
  ; Stilles Update (aus der App): Desktop-Symbol nur, wenn es schon vorher da war
  ${If} ${Silent}
    SetShellVarContext all
    ${IfNot} ${FileExists} "$DESKTOP\${APP}.lnk"
      SectionSetFlags ${SecDesktop} 0
    ${EndIf}
    ; Vorhandenen Installationsordner beibehalten
    ReadRegStr $0 HKLM "Software\${APP}" "InstallDir"
    ${If} $0 != ""
      StrCpy $INSTDIR $0
    ${EndIf}
  ${EndIf}
FunctionEnd

; Nach einem Update aus der App heraus: NebulaSSH ohne Adminrechte neu starten
Function .onInstSuccess
  ${GetParameters} $R0
  ClearErrors
  ${GetOptions} $R0 "/RELAUNCH" $R1
  ${IfNot} ${Errors}
    Exec '"$WINDIR\explorer.exe" "$INSTDIR\${EXE}"'
  ${EndIf}
FunctionEnd

LangString DESC_Main ${LANG_GERMAN} "Das Programm selbst."
LangString DESC_Desktop ${LANG_GERMAN} "Legt eine Verknüpfung auf dem Desktop an."
!insertmacro MUI_FUNCTION_DESCRIPTION_BEGIN
  !insertmacro MUI_DESCRIPTION_TEXT ${SecMain} $(DESC_Main)
  !insertmacro MUI_DESCRIPTION_TEXT ${SecDesktop} $(DESC_Desktop)
!insertmacro MUI_FUNCTION_DESCRIPTION_END

Function un.onInit
  SetRegView 64
FunctionEnd

Section "Uninstall"
  nsExec::Exec 'taskkill /IM ${EXE} /F'
  Sleep 300
  SetShellVarContext all
  Delete "$SMPROGRAMS\${APP}.lnk"
  Delete "$DESKTOP\${APP}.lnk"
  Delete "$INSTDIR\${EXE}"
  Delete "$INSTDIR\NebulaSSH.ico"
  Delete "$INSTDIR\Uninstall.exe"
  RMDir "$INSTDIR"
  DeleteRegKey HKLM "${UNINST_KEY}"
  DeleteRegKey HKLM "Software\${APP}"

  ; Benutzerdaten nur auf ausdrücklichen Wunsch entfernen
  SetShellVarContext current
  IfSilent done
  MessageBox MB_YESNO|MB_ICONQUESTION|MB_DEFBUTTON2 \
    "Sollen auch deine gespeicherten Geräte, SSH-Keys und Einstellungen gelöscht werden?$\r$\n$\r$\n($APPDATA\${APP})$\r$\n$\r$\nTipp: Vorher ein Backup erstellen, falls du sie noch brauchst." \
    IDNO done
  RMDir /r "$APPDATA\${APP}"
  done:
SectionEnd
