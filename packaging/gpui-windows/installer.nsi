Unicode true
!include "MUI2.nsh"
!include "LogicLib.nsh"
!include "WinVer.nsh"
!include "x64.nsh"

!ifndef PACKAGE_VERSION
!error "PACKAGE_VERSION is required"
!endif
!ifndef PAYLOAD_DIR
!error "PAYLOAD_DIR is required"
!endif
!ifndef OUTPUT_FILE
!error "OUTPUT_FILE is required"
!endif

Name "XSCC Controller Beta (unsigned)"
OutFile "${OUTPUT_FILE}"
InstallDir "$LOCALAPPDATA\Programs\XSCC-Controller"
RequestExecutionLevel user
ManifestSupportedOS all
SetCompressor /SOLID lzma
ShowInstDetails show
ShowUninstDetails show
!define UNINSTALL_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\XSCC-Controller"

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_LICENSE "${PAYLOAD_DIR}\LICENCE"
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"

Function .onInit
  SetShellVarContext current
  ${IfNot} ${RunningX64}
    MessageBox MB_ICONSTOP "This preview requires Windows 11 x86_64."
    Abort
  ${EndIf}
  ReadEnvStr $0 "PROCESSOR_ARCHITEW6432"
  ${If} $0 == ""
    ReadEnvStr $0 "PROCESSOR_ARCHITECTURE"
  ${EndIf}
  ${If} $0 != "AMD64"
    MessageBox MB_ICONSTOP "Only x86_64 Windows is supported by this preview."
    Abort
  ${EndIf}
  ${IfNot} ${AtLeastWin11}
    MessageBox MB_ICONSTOP "This preview requires Windows 11 (build 22000 or newer)."
    Abort
  ${EndIf}
FunctionEnd

Section "XSCC Controller Beta"
  SetShellVarContext current
  SetRegView 64
  SetOutPath "$INSTDIR"
  File "${PAYLOAD_DIR}\xscc-controller.exe"
  File "${PAYLOAD_DIR}\LICENCE"
  File "${PAYLOAD_DIR}\README.txt"
  File "${PAYLOAD_DIR}\build-info.json"
  File "${PAYLOAD_DIR}\dependencies.txt"
  WriteUninstaller "$INSTDIR\Uninstall.exe"
  CreateDirectory "$SMPROGRAMS\XSCC Controller Beta"
  CreateShortcut "$SMPROGRAMS\XSCC Controller Beta\XSCC Controller Beta.lnk" "$INSTDIR\xscc-controller.exe"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayName" "XSCC Controller Beta (unsigned)"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayVersion" "${PACKAGE_VERSION}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "UninstallString" '$\"$INSTDIR\Uninstall.exe$\"'
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoRepair" 1
SectionEnd

Section "Uninstall"
  SetShellVarContext current
  SetRegView 64
  Delete "$SMPROGRAMS\XSCC Controller Beta\XSCC Controller Beta.lnk"
  RMDir "$SMPROGRAMS\XSCC Controller Beta"
  Delete "$INSTDIR\xscc-controller.exe"
  Delete "$INSTDIR\LICENCE"
  Delete "$INSTDIR\README.txt"
  Delete "$INSTDIR\build-info.json"
  Delete "$INSTDIR\dependencies.txt"
  Delete "$INSTDIR\Uninstall.exe"
  RMDir "$INSTDIR"
  DeleteRegKey HKCU "${UNINSTALL_KEY}"
SectionEnd
