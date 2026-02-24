!macro NSIS_HOOK_PREINSTALL
  nsExec::ExecToLog 'taskkill /IM rootcx-core.exe /F /T'
  Sleep 500
!macroend

!macro NSIS_HOOK_POSTINSTALL
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  nsExec::ExecToLog 'schtasks /End /TN "RootCX\rootcx-core"'
  nsExec::ExecToLog 'schtasks /Delete /TN "RootCX\rootcx-core" /F'
  ; /T kills child tree (embedded PostgreSQL)
  nsExec::ExecToLog 'taskkill /IM rootcx-core.exe /F /T'
  Sleep 1000
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  RMDir /r "$PROFILE\.rootcx"
  RMDir /r "$APPDATA\RootCX"
  RMDir /r "$APPDATA\rootcx"
  RMDir /r "$LOCALAPPDATA\rootcx"
!macroend
