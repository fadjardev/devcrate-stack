@echo off
REM ============================================================
REM  phpuse.bat - switch the global CLI PHP version
REM  Usage:  phpuse 74 | 82 | 85        (or:  phpuse   to show)
REM  Works by re-pointing the junction  <stack-root>\php\current
REM  which is what your PATH points at. The stack root is
REM  resolved from this script's own location.
REM ============================================================
setlocal
set "PHPROOT=%~dp0php"

if "%~1"=="" goto :show

set TARGET=%PHPROOT%\php%~1
if not exist "%TARGET%\php.exe" (
    echo [ERROR] %TARGET%\php.exe not found.
    echo Installed versions:
    for /d %%D in ("%PHPROOT%\php*") do echo    %%~nxD
    exit /b 1
)

if exist "%PHPROOT%\current" rmdir "%PHPROOT%\current"
mklink /J "%PHPROOT%\current" "%TARGET%" >nul
if errorlevel 1 (
    echo [ERROR] Could not update junction.
    exit /b 1
)
echo Switched global PHP -^> php%~1
"%PHPROOT%\current\php.exe" -v
exit /b 0

:show
echo Active PHP:
"%PHPROOT%\current\php.exe" -v
echo.
echo Installed versions:
for /d %%D in ("%PHPROOT%\php*") do echo    %%~nxD
exit /b 0
