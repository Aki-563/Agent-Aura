@echo off
rem Signs one file the installer build hands over (tauri.conf.json: bundle.windows.signCommand).
rem
rem Set AURA_SIGN_THUMBPRINT to the SHA-1 thumbprint of a code-signing certificate in the current
rem user's certificate store to sign agent-aura.exe, aura-cli.exe and the installer. Without it the
rem build still succeeds, unsigned (Windows SmartScreen and some antivirus programs then warn).
rem AURA_SIGN_TIMESTAMP may name another RFC 3161 timestamp server.

if "%AURA_SIGN_THUMBPRINT%"=="" (
  echo [sign] AURA_SIGN_THUMBPRINT is not set: %~nx1 stays unsigned.
  exit /b 0
)
if "%AURA_SIGN_TIMESTAMP%"=="" set AURA_SIGN_TIMESTAMP=http://timestamp.digicert.com

where signtool >nul 2>nul
if errorlevel 1 (
  echo [sign] signtool.exe was not found on PATH ^(it comes with the Windows SDK^).
  exit /b 1
)
signtool sign /sha1 %AURA_SIGN_THUMBPRINT% /fd sha256 /tr %AURA_SIGN_TIMESTAMP% /td sha256 /d "Agent Aura" "%~1"
