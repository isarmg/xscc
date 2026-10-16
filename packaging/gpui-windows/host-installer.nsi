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

Name "XSCC Host Beta (unsigned)"
OutFile "${OUTPUT_FILE}"
InstallDir "$PROGRAMFILES64\XSCC-Host"
RequestExecutionLevel admin
ManifestSupportedOS all
SetCompressor /SOLID lzma
ShowInstDetails show
ShowUninstDetails show
!define UNINSTALL_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\XSCC-Host"
!define MUI_WELCOMEPAGE_TEXT "This unsigned beta installs XSCC Host and registers a LocalSystem Windows service. The host UI requires administrator approval. No drivers or firewall/security changes are installed. Close the host UI before upgrade or removal. Host identity and configuration are preserved on uninstall."
!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_LICENSE "${PAYLOAD_DIR}\LICENCE"
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"

Function .onInit
  SetShellVarContext all
  IfFileExists "$INSTDIR\xscc-host.exe" 0 fresh_install
    MessageBox MB_ICONSTOP "An existing XSCC Host installation was found. Uninstall it first; identity and configuration are preserved."
    Abort
  fresh_install:
  ${IfNot} ${RunningX64}
    MessageBox MB_ICONSTOP "This beta requires Windows 11 x86_64."
    Abort
  ${EndIf}
  ReadEnvStr $0 "PROCESSOR_ARCHITEW6432"
  ${If} $0 == ""
    ReadEnvStr $0 "PROCESSOR_ARCHITECTURE"
  ${EndIf}
  ${If} $0 != "AMD64"
    MessageBox MB_ICONSTOP "Only x86_64 Windows is supported by this beta."
    Abort
  ${EndIf}
  ${IfNot} ${AtLeastWin11}
    MessageBox MB_ICONSTOP "This beta requires Windows build 22000 or newer."
    Abort
  ${EndIf}
FunctionEnd

Section "XSCC Host"
  SetShellVarContext all
  SetRegView 64
  SetOutPath "$INSTDIR"
  ClearErrors
  File "${PAYLOAD_DIR}\xscc-host.exe"
  File "${PAYLOAD_DIR}\LICENCE"
  File "${PAYLOAD_DIR}\THIRD_PARTY_NOTICES.txt"
  File "${PAYLOAD_DIR}\GPUI-LICENSE-APACHE.txt"
  File "${PAYLOAD_DIR}\README.txt"
  File "${PAYLOAD_DIR}\build-info.json"
  File "${PAYLOAD_DIR}\dependencies.txt"
  ${If} ${Errors}
    SetErrorLevel 1
    Abort "Could not install all package files."
  ${EndIf}
  WriteUninstaller "$INSTDIR\Uninstall.exe"
  ExecWait '$\"$INSTDIR\xscc-host.exe$\" --install-service-only' $0
  ${If} $0 != 0
    MessageBox MB_ICONSTOP "Host service registration failed (code $0). Files have been retained for diagnosis. No driver or firewall changes were requested."
    SetErrorLevel 1
    Abort
  ${EndIf}
  CreateDirectory "$SMPROGRAMS\XSCC Host"
  CreateShortcut "$SMPROGRAMS\XSCC Host\XSCC Host.lnk" "$INSTDIR\xscc-host.exe"
  WriteRegStr HKLM "${UNINSTALL_KEY}" "DisplayName" "XSCC Host Beta (unsigned)"
  WriteRegStr HKLM "${UNINSTALL_KEY}" "DisplayVersion" "${PACKAGE_VERSION}"
  WriteRegStr HKLM "${UNINSTALL_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKLM "${UNINSTALL_KEY}" "UninstallString" '$\"$INSTDIR\Uninstall.exe$\"'
  WriteRegDWORD HKLM "${UNINSTALL_KEY}" "NoModify" 1
  WriteRegDWORD HKLM "${UNINSTALL_KEY}" "NoRepair" 1
SectionEnd

Section "Uninstall"
  SetShellVarContext all
  SetRegView 64
  ExecWait '$\"$INSTDIR\xscc-host.exe$\" --uninstall-service-only' $0
  ${If} $0 != 0
    MessageBox MB_ICONSTOP "Host service removal failed (code $0). Files and configuration have been retained."
    SetErrorLevel 1
    Abort
  ${EndIf}
  Delete "$SMPROGRAMS\XSCC Host\XSCC Host.lnk"
  RMDir "$SMPROGRAMS\XSCC Host"
  Delete "$INSTDIR\xscc-host.exe"
  Delete "$INSTDIR\LICENCE"
  Delete "$INSTDIR\THIRD_PARTY_NOTICES.txt"
  Delete "$INSTDIR\GPUI-LICENSE-APACHE.txt"
  Delete "$INSTDIR\README.txt"
  Delete "$INSTDIR\build-info.json"
  Delete "$INSTDIR\dependencies.txt"
  Delete "$INSTDIR\Uninstall.exe"
  RMDir "$INSTDIR"
  DeleteRegKey HKLM "${UNINSTALL_KEY}"
SectionEnd
