@echo off
setlocal

REM Resolve the stack root from this script's own location (works anywhere).
set "ROOT=%~dp0"
set "ROOT=%ROOT:~0,-1%"
REM What :kill_under compares process paths against (see the end of the file).
set "DEVCRATE_KILL_ROOT=%ROOT%"

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
call :kill_under php-cgi

echo Gracefully stopping RabbitMQ ...
set "ERLANG_HOME=%ROOT%\erlang"
set "RABBITMQ_BASE=%ROOT%\rabbitmq\data"
set "PATH=%ROOT%\erlang\bin;%PATH%"
if exist "%ROOT%\rabbitmq\sbin\rabbitmqctl.bat" (
    call "%ROOT%\rabbitmq\sbin\rabbitmqctl.bat" stop 2>nul
    timeout /t 3 /nobreak > nul
)
call :kill_under erl
call :kill_under epmd
call :kill_under inet_gethost
call :kill_under win32sysinfo

echo Gracefully stopping PostgreSQL ...
if exist "%ROOT%\postgres\bin\pg_ctl.exe" if exist "%ROOT%\postgres\data\postmaster.pid" (
    REM -m fast, as `devcrate stop` does: roll back open transactions, do not wait on them.
    "%ROOT%\postgres\bin\pg_ctl.exe" -D "%ROOT%\postgres\data" -m fast stop >nul 2>&1
)

echo Gracefully stopping Redis ...
if exist "%ROOT%\redis\redis-cli.exe" (
    "%ROOT%\redis\redis-cli.exe" -p 6379 shutdown >nul 2>&1
)

echo Stopping Mailpit ...
REM Mailpit has no shutdown command; `devcrate stop` terminates it too.
call :kill_under mailpit

echo Gracefully stopping MariaDB ...
if exist "%ROOT%\mariadb\bin\mariadb-admin.exe" (
    "%ROOT%\mariadb\bin\mariadb-admin.exe" --defaults-file="%ROOT%\mariadb\my.ini" -u root shutdown 2>nul
    timeout /t 3 /nobreak > nul
)
call :kill_under mariadbd

echo.
echo All services stopped.
goto :eof

REM ---------------------------------------------------------------------------
REM :kill_under <image> -- terminate the processes of that name whose executable
REM lives under this stack root, and only those. `taskkill /IM` would take every
REM php-cgi or mariadbd on the machine, another stack's or a system service's
REM included; this is the path match `devcrate stop` uses. The root reaches
REM PowerShell through an environment variable rather than the command line, so
REM a path with & ' or ! in it cannot break the command.
:kill_under
powershell -NoProfile -NonInteractive -Command "$root = $env:DEVCRATE_KILL_ROOT.TrimEnd('\') + '\'; Get-Process -Name '%~1' -ErrorAction SilentlyContinue | Where-Object { $_.Path -and $_.Path.StartsWith($root, [StringComparison]::OrdinalIgnoreCase) } | ForEach-Object { Stop-Process -Id $_.Id -Force -ErrorAction SilentlyContinue; '  terminated ' + $_.Name + ' (pid ' + $_.Id + ')' }"
goto :eof
