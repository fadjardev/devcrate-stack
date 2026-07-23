@echo off
REM ============================================================
REM  phpuse.bat - switch the global CLI PHP version
REM  Usage:  phpuse 8.5 | 85 | php-8.5      (or:  phpuse   to show)
REM  Works by re-pointing the junction  <stack-root>\php\current
REM  which is what your PATH points at. The stack root is
REM  resolved from this script's own location.
REM ============================================================
setlocal enabledelayedexpansion
set "PHPROOT=%~dp0php"

if "%~1"=="" goto :show

REM Match on the version digits alone, so 8.5, 85 and php-8.5 all name the
REM same folder -- the same spellings "devcrate php use" accepts, which keeps
REM scripts written against the old php85 folder names working.
call :digits WANT "%~1"

set "TARGET="
set "NAME="
for /d %%D in ("%PHPROOT%\php-*") do (
    call :digits HAVE "%%~nxD"
    if /i "!HAVE!"=="!WANT!" (
        set "TARGET=%%~fD"
        set "NAME=%%~nxD"
    )
)

if not defined TARGET goto :unknown
if not exist "!TARGET!\php.exe" goto :unknown

if exist "%PHPROOT%\current" rmdir "%PHPROOT%\current"
mklink /J "%PHPROOT%\current" "!TARGET!" >nul
if errorlevel 1 (
    echo [ERROR] Could not update junction.
    exit /b 1
)
echo Switched global PHP -^> !NAME!
"%PHPROOT%\current\php.exe" -v
exit /b 0

:unknown
echo [ERROR] PHP %~1 is not installed under %PHPROOT%.
echo Installed versions:
for /d %%D in ("%PHPROOT%\php-*") do echo    %%~nxD
exit /b 1

:show
echo Active PHP:
"%PHPROOT%\current\php.exe" -v
echo.
echo Installed versions:
for /d %%D in ("%PHPROOT%\php-*") do echo    %%~nxD
exit /b 0

REM Reduce a version spelling to its digits: php-8.5 / 8.5 / 85 -^> 85
:digits
set "_D=%~2"
set "_D=%_D:.=%"
set "_D=%_D:php=%"
set "_D=%_D:-=%"
set "%~1=%_D%"
exit /b 0
