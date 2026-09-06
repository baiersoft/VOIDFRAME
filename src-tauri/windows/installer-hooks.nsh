!macro NSIS_HOOK_PREINSTALL
  ; Kill both bundled tools before file copy so an upgrade install never
  ; hits a locked-file error -- HWiNFO in particular can stay resident
  ; (docs/06-driver-and-thermal.md "process lifecycle").
  nsExec::Exec '"$SYSDIR\taskkill.exe" /F /IM PresentMon.exe'
  Pop $0
  nsExec::Exec '"$SYSDIR\taskkill.exe" /F /IM HWiNFO64.exe'
  Pop $0
  Sleep 500
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  ; Tauri's own "delete application data" checkbox only clears
  ; $APPDATA/$LOCALAPPDATA\${BUNDLEID} (i.e. com.baiersoft.voidframe) --
  ; VOIDFRAME never writes there. Real data lives at the path
  ; DataRoot::resolve() actually uses (crates/voidframe-engine/src/paths.rs):
  ; %LOCALAPPDATA%\baiersoft\VOIDFRAME. Same checkbox-state/update-mode
  ; guard the built-in cleanup above this hook already used.
  ${If} $DeleteAppDataCheckboxState = 1
  ${AndIf} $UpdateMode <> 1
    SetShellVarContext current
    RMDir /r "$LOCALAPPDATA\baiersoft\VOIDFRAME"
    ; Non-recursive RMDir only removes an already-empty directory and is a
    ; harmless no-op otherwise -- if some other baiersoft app ever shares
    ; this parent folder, its contents are left untouched.
    RMDir "$LOCALAPPDATA\baiersoft"
  ${EndIf}
!macroend
