@echo off
setlocal

REM Resolve the stack root from this script's own location (works anywhere).
set "ROOT=%~dp0"
set "ROOT=%ROOT:~0,-1%"

echo === Devcrate  -  PHP Development Stack (%ROOT%)  -  STOP ===
echo.

echo Gracefully stopping Nginx (waiting for active connections) ...
REM Same prefix/binary resolution as start.bat -- see the comment there.
set "NGX_PREFIX="
if exist "%ROOT%\nginx\conf\nginx.conf" set "NGX_PREFIX=%ROOT%\nginx"
if not defined NGX_PREFIX for /d %%D in ("%ROOT%\nginx-*") do if exist "%%D\conf\nginx.conf" set "NGX_PREFIX=%%D"

set "NGX_EXE="
if defined NGX_PREFIX (
    if exist "%NGX_PREFIX%\current\nginx.exe" set "NGX_EXE=%NGX_PREFIX%\current\nginx.exe"
)
if defined NGX_PREFIX if not defined NGX_EXE (
    if exist "%NGX_PREFIX%\nginx.exe" set "NGX_EXE=%NGX_PREFIX%\nginx.exe"
)
if defined NGX_PREFIX if not defined NGX_EXE (
    for /d %%D in ("%NGX_PREFIX%\nginx-*") do if exist "%%D\nginx.exe" set "NGX_EXE=%%D\nginx.exe"
)

if defined NGX_EXE (
    "%NGX_EXE%" -p "%NGX_PREFIX%" -s quit
    timeout /t 4 /nobreak > nul
) else (
    echo   SKIPPED - no nginx.exe found
)

echo Terminating PHP-CGI processes ...
taskkill /F /IM php-cgi.exe /T 2>nul

echo Gracefully stopping RabbitMQ ...
set "ERLANG_HOME=%ROOT%\erlang"
set "RABBITMQ_BASE=%ROOT%\rabbitmq\data"
set "PATH=%ROOT%\erlang\bin;%PATH%"
if exist "%ROOT%\rabbitmq\sbin\rabbitmqctl.bat" (
    call "%ROOT%\rabbitmq\sbin\rabbitmqctl.bat" stop 2>nul
    timeout /t 3 /nobreak > nul
)
taskkill /F /IM erl.exe /T 2>nul
taskkill /F /IM epmd.exe /T 2>nul

echo Gracefully stopping MariaDB ...
if exist "%ROOT%\mariadb\bin\mariadb-admin.exe" (
    "%ROOT%\mariadb\bin\mariadb-admin.exe" --defaults-file="%ROOT%\mariadb\my.ini" -u root shutdown 2>nul
    timeout /t 3 /nobreak > nul
)
taskkill /F /IM mariadbd.exe /T 2>nul

echo.
echo All services stopped.
