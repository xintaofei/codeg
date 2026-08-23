; Tauri NSIS installer hooks.
;
; codeg-mcp.exe is the MCP stdio companion spawned by each agent CLI
; (claude / codex / opencode / ...), which is itself a grandchild of
; codeg.exe. Windows does not propagate parent death to descendants the
; way Unix does, so stale codeg-mcp.exe processes from a previous session
; can keep the binary file locked. The installer then fails to overwrite
; it with:
;
;     Error opening file for writing: ...\codeg\codeg-mcp.exe
;
; Stop any running companion processes before the installer writes new
; binaries (or removes the existing ones on uninstall). taskkill returns
; non-zero when no processes match, which is fine — we ignore the result.

!macro NSIS_HOOK_PREINSTALL
  DetailPrint "Stopping any running codeg-mcp processes..."
  nsExec::Exec 'taskkill /F /T /IM codeg-mcp.exe'
  Pop $0
  ; Small grace period so the OS releases file handles before the
  ; installer attempts to overwrite codeg-mcp.exe.
  Sleep 500
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  DetailPrint "Stopping any running codeg-mcp processes..."
  nsExec::Exec 'taskkill /F /T /IM codeg-mcp.exe'
  Pop $0
  Sleep 500
!macroend

; Deliberately NOT cleaning up the "launch at login" HKCU Run values here.
; Tauri's installer template inserts NSIS_HOOK_PREUNINSTALL unconditionally at
; the top of `Section Uninstall`, and an upgrading install runs the *previous*
; version's uninstaller (`ExecWait` in the `reinst_uninstall` branch) — so a
; DeleteRegValue in this hook would silently turn launch-at-login off on every
; update, not just on a real uninstall. `$UpdateMode` only distinguishes the two
; when the in-app updater drove it; a manually re-run installer looks like a
; plain uninstall. Leaving a stale Run value behind after an uninstall is
; cosmetic (Windows ignores an entry whose target is gone), so it wins over
; losing the user's setting on upgrade.
