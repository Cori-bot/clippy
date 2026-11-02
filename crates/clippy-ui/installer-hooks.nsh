; Extra steps for the NSIS installer Tauri generates (see tauri.installer.conf.json).
;
; The template only looks for the app's main binary (clippy-ui.exe) when it
; checks that nothing is running. Clippy is three executables, and the other
; two are the ones that matter here: clippy-daemon.exe holds the capture
; session and its files open, and the installer cannot replace or remove a
; running exe.

; Politely first (WM_CLOSE reaches the daemon's message window), then by force.
; Each nsExec::Exec leaves its exit code on the stack; it is popped and
; ignored, because "not running" is an exit code of 128 and is not an error.
!macro ClippyStopProcesses
  Push $0
  nsExec::Exec 'taskkill /IM clippy-daemon.exe'
  Pop $0
  nsExec::Exec 'taskkill /IM clippy.exe'
  Pop $0
  Sleep 1500
  nsExec::Exec 'taskkill /F /IM clippy-daemon.exe'
  Pop $0
  nsExec::Exec 'taskkill /F /IM clippy.exe'
  Pop $0
  Sleep 300
  Pop $0
!macroend

!macro NSIS_HOOK_PREINSTALL
  !insertmacro ClippyStopProcesses
  ; A remembered install folder under %TEMP% is the leftover of a scripted
  ; install test (scripts\build-installer.ps1), never a real choice: an
  ; interactive install goes to the default folder instead. A silent install
  ; keeps its /D= target, which is how that test installs.
  IfSilent clippy_dir_ok
  Push $1
  Push $2
  StrLen $1 $TEMP
  StrCpy $2 $INSTDIR $1
  StrCmp $2 $TEMP 0 +2
  StrCpy $INSTDIR "$LOCALAPPDATA\Clippy"
  Pop $2
  Pop $1
  clippy_dir_ok:
!macroend

!macro NSIS_HOOK_POSTINSTALL
  ; Explorer keeps shortcut icons in its own cache: after an update that
  ; changes the logo, the desktop shortcut kept showing the old one. This is
  ; the documented "icons changed" broadcast (SHCNE_ASSOCCHANGED) that makes
  ; it redraw them from the exe.
  System::Call 'shell32::SHChangeNotify(i 0x08000000, i 0, p 0, p 0)'
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  !insertmacro ClippyStopProcesses
  ; Leftovers of the in-app updater, which swaps binaries by renaming the old
  ; ones to <name>.old; the uninstaller only removes files it installed.
  Delete "$INSTDIR\*.old"
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  ; The opt-in "start with Windows" entry (clippy-daemon's autostart.rs) points
  ; into the folder that was just removed.
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "Clippy"
!macroend
