; Astrofin — NSIS installer (per-user, no elevation)
;
; Built by dev\windows\package.ps1, which passes the payload directory, the
; version strings and the output path in via /D defines. Do not run makensis on
; this file directly unless you supply all of them.
;
;   makensis /DAPP_VERSION=... /DAPP_VERSION_NUM=a.b.c.0 \
;            /DPAYLOAD_DIR=<abs> /DICON_FILE=<abs> /DOUT_FILE=<abs> astrofin.nsi
;
; Design notes:
;   * Per-user only. InstallDir is %LOCALAPPDATA%\Programs\Astrofin, every
;     registry write goes to HKCU and RequestExecutionLevel is `user`, so the
;     installer never triggers a UAC prompt.
;   * The whole staged tree is packaged recursively (File /r) because the CEF
;     runtime is ~130 DLLs plus locales\ and .pak/.dat resources.
;   * Uninstall removes only $INSTDIR and the shortcuts. The user profile in
;     %APPDATA%\astrofin and %LOCALAPPDATA%\astrofin is deliberately left alone
;     and is not even offered as an option.

Unicode true
ManifestDPIAware true

!include "MUI2.nsh"
!include "LogicLib.nsh"
!include "FileFunc.nsh"
!include "WordFunc.nsh"

;--------------------------------------------------------------------------
; Required defines (supplied by package.ps1)
;--------------------------------------------------------------------------
!ifndef APP_VERSION
  !error "APP_VERSION is required (full display version, e.g. 0.1.0-dev+abc1234)"
!endif
!ifndef APP_VERSION_NUM
  !error "APP_VERSION_NUM is required (numeric a.b.c.0 for VIProductVersion)"
!endif
!ifndef PAYLOAD_DIR
  !error "PAYLOAD_DIR is required (absolute path to the staged tree)"
!endif
!ifndef OUT_FILE
  !error "OUT_FILE is required (absolute path of the installer to write)"
!endif
!ifndef ICON_FILE
  !error "ICON_FILE is required (absolute path to astrofin.ico)"
!endif
!ifndef COMPRESSOR
  !define COMPRESSOR "lzma"
!endif

;--------------------------------------------------------------------------
; Product identity
;--------------------------------------------------------------------------
!define APP_NAME      "Astrofin"
!define APP_PUBLISHER "TheHalfrican"
!define APP_EXE       "astrofin.exe"
!define APP_URL       "https://github.com/TheHalfrican/Astrofin"
; Must match APP_USER_MODEL_ID in src/windows/src/platform.rs, otherwise the
; taskbar will not group the running process with its pinned shortcut and SMTC
; will not associate the media session with the app.
!define APP_AUMID     "io.github.thehalfrican.Astrofin"
!define UNINST_KEY    "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP_NAME}"
!define SETTINGS_KEY  "Software\${APP_PUBLISHER}\${APP_NAME}"

Name "${APP_NAME}"
OutFile "${OUT_FILE}"
InstallDir "$LOCALAPPDATA\Programs\${APP_NAME}"
InstallDirRegKey HKCU "${SETTINGS_KEY}" "InstallDir"
RequestExecutionLevel user
SetCompressor /SOLID ${COMPRESSOR}
XPStyle on
BrandingText "${APP_NAME} ${APP_VERSION}"

VIProductVersion "${APP_VERSION_NUM}"
VIAddVersionKey "ProductName"     "${APP_NAME}"
VIAddVersionKey "CompanyName"     "${APP_PUBLISHER}"
VIAddVersionKey "LegalCopyright"  "${APP_PUBLISHER}"
VIAddVersionKey "FileDescription" "${APP_NAME} setup"
VIAddVersionKey "FileVersion"     "${APP_VERSION}"
VIAddVersionKey "ProductVersion"  "${APP_VERSION}"

;--------------------------------------------------------------------------
; UI
;--------------------------------------------------------------------------
!define MUI_ICON   "${ICON_FILE}"
!define MUI_UNICON "${ICON_FILE}"
!define MUI_ABORTWARNING

!insertmacro MUI_PAGE_COMPONENTS
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!define MUI_FINISHPAGE_RUN "$INSTDIR\${APP_EXE}"
!define MUI_FINISHPAGE_RUN_NOTCHECKED
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

!insertmacro MUI_LANGUAGE "English"

;--------------------------------------------------------------------------
; Shell-link AppUserModelID
;--------------------------------------------------------------------------
; NSIS's CreateShortcut cannot write shell-link property-store values, so the
; .lnk is re-opened through IPersistFile/IPropertyStore and stamped with
; System.AppUserModel.ID.  Called with $R8 = .lnk path, $R9 = AUMID.
!define CLSID_ShellLink    {00021401-0000-0000-C000-000000000046}
!define IID_IPersistFile   {0000010B-0000-0000-C000-000000000046}
!define IID_IPropertyStore {886D8EEB-8CF2-4446-8D02-CDBA1DBDCF99}
!define STGM_READWRITE     0x00000002

!macro StampAumid UN
Function ${UN}StampAumid
  Push $0 ; IPersistFile*
  Push $1 ; IPropertyStore*
  Push $2 ; PROPERTYKEY buffer
  Push $3 ; PROPVARIANT buffer
  Push $4 ; HRESULT
  Push $5 ; .lnk path      (System::Call cannot name $R8/$R9 directly)
  Push $6 ; AUMID
  Push $7 ; CoTaskMem copy of the AUMID

  StrCpy $5 $R8
  StrCpy $6 $R9
  StrCpy $0 0
  StrCpy $1 0
  System::Call 'ole32::CoInitialize(p 0)'
  System::Call 'ole32::CoCreateInstance(g "${CLSID_ShellLink}", p 0, i 1, \
      g "${IID_IPersistFile}", *p 0 r0) i .r4'
  ${If} $4 = 0
  ${AndIf} $0 <> 0
    System::Call '$0->5(w r5, i ${STGM_READWRITE}) i .r4' ; IPersistFile::Load
    ${If} $4 = 0
      System::Call '$0->0(g "${IID_IPropertyStore}", *p 0 r1) i .r4' ; QueryInterface
      ${If} $4 = 0
      ${AndIf} $1 <> 0
        System::Alloc 32 ; PROPERTYKEY (GUID + DWORD)
        Pop $2
        System::Alloc 32 ; PROPVARIANT
        Pop $3
        System::Call 'propsys::PSGetPropertyKeyFromName(w "System.AppUserModel.ID", p r2) i .r4'
        ${If} $4 = 0
          ; PROPVARIANT{ vt = VT_LPWSTR (31), 3 reserved WORDs, pwszVal }.
          ; InitPropVariantFromString is an inline helper in propvarutil.h, not
          ; a propsys.dll export, so the struct is filled in by hand. SHStrDupW
          ; allocates with CoTaskMemAlloc, which is what PropVariantClear frees.
          System::Call 'shlwapi::SHStrDupW(w r6, *p 0 r7) i .r4'
          ${If} $4 = 0
            System::Call '*$3(&i2 31, &i2 0, &i2 0, &i2 0, p r7)'
            System::Call '$1->6(p r2, p r3) i .r4' ; IPropertyStore::SetValue
            System::Call '$1->7() i .r4'           ; IPropertyStore::Commit
            System::Call 'ole32::PropVariantClear(p r3)'
          ${EndIf}
        ${EndIf}
        System::Free $3
        System::Free $2
        System::Call '$1->2()' ; Release
      ${EndIf}
      System::Call '$0->6(p 0, i 1) i .r4' ; IPersistFile::Save(NULL, TRUE)
    ${EndIf}
    System::Call '$0->2()' ; Release
  ${EndIf}
  System::Call 'ole32::CoUninitialize()'
  ${If} $4 <> 0
    DetailPrint "warning: could not set System.AppUserModel.ID on $5 (hr=$4)"
  ${EndIf}

  Pop $7
  Pop $6
  Pop $5
  Pop $4
  Pop $3
  Pop $2
  Pop $1
  Pop $0
FunctionEnd
!macroend
!insertmacro StampAumid ""

;--------------------------------------------------------------------------
; Install
;--------------------------------------------------------------------------
Var WantDesktop

Section "${APP_NAME} (required)" SEC_APP
  SectionIn RO
  SetOutPath "$INSTDIR"
  SetOverwrite on
  ; Package the entire staged tree; the CEF runtime is far too many files
  ; (locales\, *.pak, ~130 DLLs) to list individually.
  File /r "${PAYLOAD_DIR}\*.*"

  WriteRegStr HKCU "${SETTINGS_KEY}" "InstallDir" "$INSTDIR"
  WriteRegStr HKCU "${SETTINGS_KEY}" "Version"    "${APP_VERSION}"

  WriteUninstaller "$INSTDIR\uninstall.exe"

  WriteRegStr   HKCU "${UNINST_KEY}" "DisplayName"     "${APP_NAME}"
  WriteRegStr   HKCU "${UNINST_KEY}" "DisplayVersion"  "${APP_VERSION}"
  WriteRegStr   HKCU "${UNINST_KEY}" "DisplayIcon"     "$INSTDIR\${APP_EXE},0"
  WriteRegStr   HKCU "${UNINST_KEY}" "Publisher"       "${APP_PUBLISHER}"
  WriteRegStr   HKCU "${UNINST_KEY}" "URLInfoAbout"    "${APP_URL}"
  WriteRegStr   HKCU "${UNINST_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr   HKCU "${UNINST_KEY}" "UninstallString"      '"$INSTDIR\uninstall.exe"'
  WriteRegStr   HKCU "${UNINST_KEY}" "QuietUninstallString" '"$INSTDIR\uninstall.exe" /S'
  WriteRegDWORD HKCU "${UNINST_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINST_KEY}" "NoRepair" 1
  ${GetSize} "$INSTDIR" "/S=0K" $0 $1 $2
  IntFmt $0 "0x%08X" $0
  WriteRegDWORD HKCU "${UNINST_KEY}" "EstimatedSize" "$0"
SectionEnd

Section "Start menu shortcut" SEC_STARTMENU
  CreateShortCut "$SMPROGRAMS\${APP_NAME}.lnk" "$INSTDIR\${APP_EXE}" "" \
      "$INSTDIR\${APP_EXE}" 0 SW_SHOWNORMAL "" "${APP_NAME}"
  StrCpy $R8 "$SMPROGRAMS\${APP_NAME}.lnk"
  StrCpy $R9 "${APP_AUMID}"
  Call StampAumid
SectionEnd

Section /o "Desktop shortcut" SEC_DESKTOP
  CreateShortCut "$DESKTOP\${APP_NAME}.lnk" "$INSTDIR\${APP_EXE}" "" \
      "$INSTDIR\${APP_EXE}" 0 SW_SHOWNORMAL "" "${APP_NAME}"
  StrCpy $R8 "$DESKTOP\${APP_NAME}.lnk"
  StrCpy $R9 "${APP_AUMID}"
  Call StampAumid
SectionEnd

!insertmacro MUI_FUNCTION_DESCRIPTION_BEGIN
  !insertmacro MUI_DESCRIPTION_TEXT ${SEC_APP} \
      "${APP_NAME} and its bundled CEF and mpv runtimes."
  !insertmacro MUI_DESCRIPTION_TEXT ${SEC_STARTMENU} \
      "Add ${APP_NAME} to the Start menu."
  !insertmacro MUI_DESCRIPTION_TEXT ${SEC_DESKTOP} \
      "Add a ${APP_NAME} shortcut to the desktop."
!insertmacro MUI_FUNCTION_DESCRIPTION_END

Function .onInit
  ; /DESKTOP=1 selects the desktop shortcut in silent installs (and pre-checks
  ; it in the UI); /DESKTOP=0 or omitting it leaves it off.
  ${GetParameters} $R0
  ClearErrors
  ${GetOptions} $R0 "/DESKTOP=" $WantDesktop
  ${If} ${Errors}
    StrCpy $WantDesktop "0"
  ${EndIf}
  ${If} $WantDesktop == "1"
    !insertmacro SelectSection ${SEC_DESKTOP}
  ${EndIf}

  Call UninstallPreviousMsi
  Call UninstallPrevious
FunctionEnd

; The MSI build of Astrofin (dev\windows\installer\astrofin.wxs) registers
; under this UpgradeCode. Both installers use the same default directory, so
; installing over an MSI would leave Windows Installer's registration pointing
; at files this package later deletes. MsiEnumRelatedProducts finds every
; installed product with the UpgradeCode in any context the current user can
; see, and a per-user product uninstalls with msiexec /x without elevation.
; Must stay in sync with UpgradeCode in astrofin.wxs (braces, upper case).
!define MSI_UPGRADE_CODE "{7D2F4C61-9A3E-4B8D-9C1F-2E6A5B0D7431}"

Function UninstallPreviousMsi
  Push $0 ; MsiEnumRelatedProducts / msiexec result
  Push $1 ; ProductCode
  Push $2 ; loop guard
  ; Always index 0: the list shrinks after each uninstall. The guard only
  ; matters if msiexec keeps failing, so the install still goes ahead.
  StrCpy $2 0
  ${Do}
    System::Call 'msi::MsiEnumRelatedProductsW(w "${MSI_UPGRADE_CODE}", i 0, i 0, w .r1) i .r0'
    ${If} $0 <> 0
      ${Break}
    ${EndIf}
    DetailPrint "Removing the MSI-installed ${APP_NAME} ($1)"
    ExecWait 'msiexec.exe /x $1 /qn' $0
    ${If} $0 <> 0
      DetailPrint "warning: msiexec /x $1 exited with $0"
    ${EndIf}
    IntOp $2 $2 + 1
  ${LoopUntil} $2 >= 3
  Pop $2
  Pop $1
  Pop $0
FunctionEnd

; Upgrade in place: run the previously installed uninstaller silently so we do
; not leave orphaned files from an older CEF/mpv drop behind. `_?=` keeps the
; uninstaller in place so ExecWait actually waits for it.
Function UninstallPrevious
  ReadRegStr $R1 HKCU "${UNINST_KEY}" "InstallLocation"
  ${If} $R1 == ""
    Return
  ${EndIf}
  ${IfNot} ${FileExists} "$R1\uninstall.exe"
    Return
  ${EndIf}
  DetailPrint "Removing the previously installed ${APP_NAME} from $R1"
  ExecWait '"$R1\uninstall.exe" /S _?=$R1' $R2
  ; The uninstaller cannot delete itself when invoked with _?=.
  Delete "$R1\uninstall.exe"
  RMDir "$R1"
FunctionEnd

;--------------------------------------------------------------------------
; Uninstall
;--------------------------------------------------------------------------
Section "Uninstall"
  Delete "$SMPROGRAMS\${APP_NAME}.lnk"
  Delete "$DESKTOP\${APP_NAME}.lnk"

  Delete "$INSTDIR\uninstall.exe"
  RMDir /r "$INSTDIR\locales"
  RMDir /r "$INSTDIR\shaders"
  Delete "$INSTDIR\*.dll"
  Delete "$INSTDIR\*.exe"
  Delete "$INSTDIR\*.pak"
  Delete "$INSTDIR\*.dat"
  Delete "$INSTDIR\*.bin"
  Delete "$INSTDIR\*.json"
  ; Anything else we shipped. $INSTDIR is a directory this installer created
  ; under %LOCALAPPDATA%\Programs, so removing it wholesale is the expected
  ; behaviour; /REBOOTOK covers a file the running app still has open.
  RMDir /r /REBOOTOK "$INSTDIR"

  DeleteRegKey HKCU "${UNINST_KEY}"
  DeleteRegKey HKCU "${SETTINGS_KEY}"

  ; NOTE: %APPDATA%\astrofin (config) and %LOCALAPPDATA%\astrofin (cache, logs)
  ; are the user's data and are intentionally NOT touched here.
SectionEnd
