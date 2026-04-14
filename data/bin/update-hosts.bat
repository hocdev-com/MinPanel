@echo off
setlocal EnableExtensions

set "SOURCE=%~1"
set "TARGET=%~2"
set "STATUS=%~3"

if defined STATUS del /f /q "%STATUS%" >nul 2>nul

if not defined SOURCE (
    call :fail Usage: update-hosts.bat ^<source-file^> [target-file] [status-file]
    exit /b 1
)

if not defined TARGET set "TARGET=%SystemRoot%\System32\drivers\etc\hosts"

if not exist "%SOURCE%" (
    call :fail Hosts update source not found: %SOURCE%
    exit /b 1
)

copy /Y "%SOURCE%" "%TARGET%" >nul || (
    call :fail Failed to update hosts file: %TARGET%
    exit /b 1
)

call :ok
exit /b 0

:ok
if defined STATUS >"%STATUS%" echo ok
exit /b 0

:fail
if defined STATUS >"%STATUS%" echo %*
>&2 echo %*
exit /b 1
