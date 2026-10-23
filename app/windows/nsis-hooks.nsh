; everytxt NSIS 安装器钩子（批次 C / M4：手册 7.12 卸载清理）
;
; tauri-bundler 按 !ifmacrodef 插入四个钩子位（PREINSTALL/POSTINSTALL/PREUNINSTALL/POSTUNINSTALL）；
; 本文件只定义卸载后清理。
;
; 数据目录实证（state.rs default_index_dir）：dirs::data_dir() 在 Windows =
; %APPDATA%（Roaming）→ 数据位于 %APPDATA%\search-tool\（含 index\ 与 app.json）。
; NSIS $APPDATA 在 SetShellVarContext current（tauri 安装器默认 per-user）下即该目录。
;
; 注意：仅当用户卸载时勾选「删除数据」才执行（自定义页面由 POSTUNINSTALL 时
; 无交互能力，这里沿用 AnyText 语义：卸载即清）。如需保留用户数据可注释本宏。

!macro NSIS_HOOK_POSTUNINSTALL
  DetailPrint "正在删除用户数据（%APPDATA%\search-tool）…"
  RMDir /r "$APPDATA\search-tool"
!macroend
