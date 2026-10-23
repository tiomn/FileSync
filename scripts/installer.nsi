; 文件同步助手 (FileSync) —— Windows 安装包脚本
;
; 依赖：NSIS 3.x（Unicode）
; 编译：
;   makensis.exe /DAPP_SRC="...\release" /DOUT_FILE="...\FileSync-Setup.exe" /DICON_FILE="...\icon.ico" installer.nsi
; 或直接运行 scripts/build-installer.ps1
;
; 说明：x86_64-pc-windows-gnu 工具链无法静态链接 WebView2 加载器，
;       可执行文件必须与 WebView2Loader.dll 同目录，故安装包会一并安装该 DLL。

Unicode true

!include "MUI2.nsh"
!include "FileFunc.nsh"
!include "LogicLib.nsh"

; ---------- 可被 /D 覆盖的参数 ----------
!ifndef APP_SRC
  !define APP_SRC "..\src-tauri\target\x86_64-pc-windows-gnu\release"
!endif
!ifndef OUT_FILE
  !define OUT_FILE "..\dist\FileSync-1.0.1-win64-Setup.exe"
!endif
!ifndef ICON_FILE
  !define ICON_FILE "..\src-tauri\icons\icon.ico"
!endif

; ---------- 应用信息 ----------
!define APP_NAME      "文件同步助手"
!define APP_EXE       "FileSync.exe"
!define APP_LOADER    "WebView2Loader.dll"
!define APP_VERSION   "1.0.1"
!define APP_PUBLISHER "FileSync"
!define APP_ID        "FileSync"
!define UNINST_KEY    "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP_ID}"
!define RUN_KEY       "Software\Microsoft\Windows\CurrentVersion\Run"
!define WEBVIEW2_GUID "{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}"

Name "${APP_NAME} ${APP_VERSION}"
Caption "${APP_NAME} ${APP_VERSION} 安装程序"
OutFile "${OUT_FILE}"
BrandingText "${APP_NAME} ${APP_VERSION}"

; 当前用户安装，无需管理员权限
RequestExecutionLevel user
InstallDir "$LOCALAPPDATA\Programs\${APP_ID}"
InstallDirRegKey HKCU "${UNINST_KEY}" "InstallLocation"

ShowInstDetails show
ShowUninstDetails show

; ---------- 版本资源 ----------
VIProductVersion "1.0.1.0"
VIAddVersionKey /LANG=2052 "ProductName"      "${APP_NAME}"
VIAddVersionKey /LANG=2052 "FileDescription"  "${APP_NAME} 安装程序"
VIAddVersionKey /LANG=2052 "CompanyName"      "${APP_PUBLISHER}"
VIAddVersionKey /LANG=2052 "LegalCopyright"   "${APP_PUBLISHER}"
VIAddVersionKey /LANG=2052 "FileVersion"      "${APP_VERSION}"
VIAddVersionKey /LANG=2052 "ProductVersion"   "${APP_VERSION}"

; ---------- 界面 ----------
!define MUI_ICON   "${ICON_FILE}"
!define MUI_UNICON "${ICON_FILE}"
!define MUI_ABORTWARNING
!define MUI_FINISHPAGE_RUN "$INSTDIR\${APP_EXE}"
!define MUI_FINISHPAGE_RUN_TEXT "立即运行 ${APP_NAME}"

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_COMPONENTS
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

!insertmacro MUI_LANGUAGE "SimpChinese"

; ---------- 安装前：环境检查 + 旧版本处理 ----------
Function .onInit
  SetShellVarContext current

  ; 1) 检测 WebView2 运行时（Windows 11 内置；Win10 一般随 Edge 安装）
  SetRegView 64
  ReadRegStr $0 HKLM "SOFTWARE\Microsoft\EdgeUpdate\Clients\${WEBVIEW2_GUID}" "pv"
  ReadRegStr $1 HKCU "SOFTWARE\Microsoft\EdgeUpdate\Clients\${WEBVIEW2_GUID}" "pv"
  SetRegView 32
  ReadRegStr $2 HKLM "SOFTWARE\Microsoft\EdgeUpdate\Clients\${WEBVIEW2_GUID}" "pv"
  SetRegView default
  ${If} $0 == ""
  ${AndIf} $1 == ""
  ${AndIf} $2 == ""
    MessageBox MB_OKCANCEL|MB_ICONEXCLAMATION "未检测到 Microsoft Edge WebView2 运行时。$\r$\n$\r$\n本程序依赖 WebView2 显示界面，缺少它时将无法启动。$\r$\n请先安装“Evergreen WebView2 Runtime”后重试。$\r$\n$\r$\n是否仍要继续安装？" IDOK +2
    Abort
  ${EndIf}

  ; 2) 已安装旧版本 -> 先静默卸载，避免残留
  ReadRegStr $R0 HKCU "${UNINST_KEY}" "UninstallString"
  ${If} $R0 != ""
    ReadRegStr $R1 HKCU "${UNINST_KEY}" "InstallLocation"
    ${If} $R1 == ""
      StrCpy $R1 "$INSTDIR"
    ${EndIf}
    ${If} ${FileExists} "$R1\uninstall.exe"
      DetailPrint "卸载已安装的旧版本：$R1"
      ClearErrors
      ExecWait '"$R1\uninstall.exe" /S _?=$R1' $R2
      Delete "$R1\uninstall.exe"
    ${EndIf}
  ${EndIf}
FunctionEnd

; ---------- 主程序 ----------
Section "${APP_NAME} 主程序" SEC_MAIN
  SectionIn RO

  ; 关闭正在运行的实例（含 WebView2 子进程），否则覆盖 exe 会失败
  DetailPrint "关闭正在运行的 ${APP_NAME} ..."
  nsExec::ExecToLog 'taskkill /IM "${APP_EXE}" /F /T'
  Pop $0
  Sleep 700

  SetOutPath "$INSTDIR"
  SetOverwrite on
  File /oname=${APP_EXE}    "${APP_SRC}\${APP_EXE}"
  File /oname=${APP_LOADER} "${APP_SRC}\${APP_LOADER}"

  ; 若用户此前开启过开机自启，Run 项里的路径要指向新安装位置
  ReadRegStr $0 HKCU "${RUN_KEY}" "${APP_ID}"
  ${If} $0 != ""
    WriteRegStr HKCU "${RUN_KEY}" "${APP_ID}" '"$INSTDIR\${APP_EXE}"'
  ${EndIf}

  WriteUninstaller "$INSTDIR\uninstall.exe"

  ; 注册到“应用和功能”
  WriteRegStr   HKCU "${UNINST_KEY}" "DisplayName"     "${APP_NAME}"
  WriteRegStr   HKCU "${UNINST_KEY}" "DisplayVersion"  "${APP_VERSION}"
  WriteRegStr   HKCU "${UNINST_KEY}" "Publisher"       "${APP_PUBLISHER}"
  WriteRegStr   HKCU "${UNINST_KEY}" "DisplayIcon"     "$INSTDIR\${APP_EXE},0"
  WriteRegStr   HKCU "${UNINST_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr   HKCU "${UNINST_KEY}" "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegStr   HKCU "${UNINST_KEY}" "QuietUninstallString" '"$INSTDIR\uninstall.exe" /S'
  WriteRegDWORD HKCU "${UNINST_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINST_KEY}" "NoRepair" 1

  ${GetSize} "$INSTDIR" "/S=0K" $1 $2 $3
  WriteRegDWORD HKCU "${UNINST_KEY}" "EstimatedSize" $1
SectionEnd

; ---------- 开始菜单快捷方式 ----------
Section "开始菜单快捷方式" SEC_STARTMENU
  CreateDirectory "$SMPROGRAMS\${APP_NAME}"
  CreateShortCut "$SMPROGRAMS\${APP_NAME}\${APP_NAME}.lnk" "$INSTDIR\${APP_EXE}" "" "$INSTDIR\${APP_EXE}" 0
  CreateShortCut "$SMPROGRAMS\${APP_NAME}\卸载 ${APP_NAME}.lnk" "$INSTDIR\uninstall.exe" "" "$INSTDIR\uninstall.exe" 0
SectionEnd

; ---------- 桌面快捷方式 ----------
Section "桌面快捷方式" SEC_DESKTOP
  CreateShortCut "$DESKTOP\${APP_NAME}.lnk" "$INSTDIR\${APP_EXE}" "" "$INSTDIR\${APP_EXE}" 0
SectionEnd

; ---------- 组件说明 ----------
!insertmacro MUI_FUNCTION_DESCRIPTION_BEGIN
  !insertmacro MUI_DESCRIPTION_TEXT ${SEC_MAIN}      "程序主体：FileSync.exe 与 WebView2Loader.dll（必需）。"
  !insertmacro MUI_DESCRIPTION_TEXT ${SEC_STARTMENU} "在开始菜单中创建快捷方式和卸载入口。"
  !insertmacro MUI_DESCRIPTION_TEXT ${SEC_DESKTOP}   "在桌面上创建快捷方式。"
!insertmacro MUI_FUNCTION_DESCRIPTION_END

; ---------- 卸载 ----------
Function un.onInit
  SetShellVarContext current
FunctionEnd

Section "Uninstall"
  nsExec::ExecToLog 'taskkill /IM "${APP_EXE}" /F /T'
  Pop $0
  Sleep 700

  ; 快捷方式
  Delete "$DESKTOP\${APP_NAME}.lnk"
  Delete "$SMPROGRAMS\${APP_NAME}\${APP_NAME}.lnk"
  Delete "$SMPROGRAMS\${APP_NAME}\卸载 ${APP_NAME}.lnk"
  RMDir  "$SMPROGRAMS\${APP_NAME}"

  ; 程序文件
  Delete "$INSTDIR\${APP_EXE}"
  Delete "$INSTDIR\${APP_LOADER}"
  Delete "$INSTDIR\uninstall.exe"
  Delete "$INSTDIR\*.log"
  RMDir  "$INSTDIR"

  ; 注册表
  DeleteRegKey   HKCU "${UNINST_KEY}"
  DeleteRegKey   HKCU "Software\Classes\AppUserModelId\${APP_ID}"
  DeleteRegValue HKCU "${RUN_KEY}" "${APP_ID}"
  DeleteRegValue HKCU "${RUN_KEY}" "filesync"
  DeleteRegValue HKCU "${RUN_KEY}" "文件同步助手"

  ; 用户数据（配置 / 历史 / 日志 / WebView2 缓存）
  MessageBox MB_YESNO|MB_ICONQUESTION|MB_DEFBUTTON2|MB_TOPMOST "是否同时删除配置与历史记录？$\r$\n$\r$\n$APPDATA\FileSync（配置、历史方案、日志）$\r$\n$LOCALAPPDATA\FileSync（界面缓存）$\r$\n$\r$\n选择“否”将保留这些数据，重新安装后可继续使用。" /SD IDNO IDNO keepdata
    RMDir /r "$APPDATA\FileSync"
    RMDir /r "$LOCALAPPDATA\FileSync"
  keepdata:
SectionEnd
