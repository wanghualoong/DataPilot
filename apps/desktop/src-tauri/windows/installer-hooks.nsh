; DataPilot NSIS hooks (installer runs elevated: perMachine).
; The kernel sandbox (AppContainer "DataPilot.Kernel") cannot reach 127.0.0.1 by default; SSH tunnels need it.
!macro NSIS_HOOK_POSTINSTALL
  nsExec::ExecToLog '"$INSTDIR\datapilot-core.exe" --loopback-exempt'
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  nsExec::ExecToLog '"$INSTDIR\datapilot-core.exe" --loopback-remove'
!macroend
