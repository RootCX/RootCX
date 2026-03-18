!macro NSIS_HOOK_PREINSTALL
!macroend

!macro NSIS_HOOK_POSTINSTALL
!macroend

!macro NSIS_HOOK_PREUNINSTALL
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  RMDir /r "$PROFILE\.rootcx"
  RMDir /r "$APPDATA\RootCX"
  RMDir /r "$APPDATA\rootcx"
  RMDir /r "$LOCALAPPDATA\rootcx"
!macroend
