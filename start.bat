@echo off
setlocal

REM Resolve the stack root from this script's own location (works anywhere).
set "ROOT=%~dp0"
set "ROOT=%ROOT:~0,-1%"

echo === Devcrate  -  PHP Development Stack (%ROOT%)  -  START ===
echo.

REM FastCGI: spawn 4 child workers per process, recycle after 500 requests
set PHP_FCGI_CHILDREN=4
set PHP_FCGI_MAX_REQUESTS=500

REM --- RabbitMQ / Erlang portable env (keeps data + erl inside the stack root) ---
set "ERLANG_HOME=%ROOT%\erlang"
set "RABBITMQ_BASE=%ROOT%\rabbitmq\data"
set "PATH=%ROOT%\erlang\bin;%PATH%"

echo [1/7] MariaDB 12.3  (port 3306) ...
if exist "%ROOT%\mariadb\bin\mariadbd.exe" (
    start /B "MariaDB" "%ROOT%\mariadb\bin\mariadbd.exe" --defaults-file="%ROOT%\mariadb\my.ini"
) else ( echo   SKIPPED - %ROOT%\mariadb\bin\mariadbd.exe not found )

echo [2/7] PHP  (every php\php-X.Y folder, port 90 + its digits) ...
REM The same rule as the binary: the folder name is the version, and the port
REM is derived from it -- php-5.6 serves on 9056, php-8.5 on 9085. Unpacking a
REM new php-X.Y folder is all it takes for this to start it.
set "PHP_FOUND="
for /d %%D in ("%ROOT%\php\php-*") do call :start_php "%%~fD"
if not defined PHP_FOUND echo   SKIPPED - no php\php-X.Y folder with php-cgi.exe under %ROOT%\php

echo [3/7] RabbitMQ 4.3.2  (ports 5672 / 15672) ...
if exist "%ROOT%\rabbitmq\sbin\rabbitmq-server.bat" (
    REM The same loopback-only conf `devcrate start` writes, unless one is there.
    if not exist "%RABBITMQ_BASE%" mkdir "%RABBITMQ_BASE%"
    if not exist "%RABBITMQ_BASE%\rabbitmq.conf" (
        echo # Written by devcrate: the broker answers this machine only.
        echo listeners.tcp.default = 127.0.0.1:5672
        echo management.tcp.ip = 127.0.0.1
    ) > "%RABBITMQ_BASE%\rabbitmq.conf"
    REM -detached: run the broker as a background Erlang node. Do NOT redirect
    REM stdout/stderr to a file -- OTP's terminal driver crashes with "nouser"
    REM when its console is a non-tty. Logs go to %RABBITMQ_BASE%\log\rabbit@HOST.log
    start /B "RabbitMQ" cmd /c ""%ROOT%\rabbitmq\sbin\rabbitmq-server.bat" -detached"
) else ( echo   SKIPPED - %ROOT%\rabbitmq\sbin\rabbitmq-server.bat not found )

REM The three below are started exactly as `devcrate start` starts them, with
REM their output discarded as it does: each keeps its own log.
echo [4/7] PostgreSQL  (port 5432) ...
if exist "%ROOT%\postgres\data\PG_VERSION" (
    REM postgres.exe itself, never pg_ctl, which forks the server and exits.
    start /B "PostgreSQL" "%ROOT%\postgres\bin\postgres.exe" -D "%ROOT%\postgres\data" -p 5432 >nul 2>&1
) else ( echo   SKIPPED - %ROOT%\postgres\data\PG_VERSION not found )

echo [5/7] Redis  (port 6379) ...
if exist "%ROOT%\redis\redis-server.exe" (
    REM devcrate.conf by a relative name, from its own folder: the msys2 build
    REM misreads a drive-letter path given on the command line.
    start /B /D "%ROOT%\redis" "Redis" "%ROOT%\redis\redis-server.exe" devcrate.conf >nul 2>&1
) else ( echo   SKIPPED - %ROOT%\redis\redis-server.exe not found )

echo [6/7] Mailpit  (SMTP 1025, inbox 8025) ...
if exist "%ROOT%\mailpit\mailpit.exe" (
    start /B /D "%ROOT%\mailpit" "Mailpit" "%ROOT%\mailpit\mailpit.exe" --listen 127.0.0.1:8025 --smtp 127.0.0.1:1025 --database data\mailpit.db --log-file data\mailpit.log --smtp-auth-accept-any --smtp-auth-allow-insecure --disable-version-check >nul 2>&1
) else ( echo   SKIPPED - %ROOT%\mailpit\mailpit.exe not found )

REM Give php-cgi a moment to bind before nginx starts proxying
timeout /t 2 /nobreak > nul

echo [7/7] Nginx ...
REM --- Resolve the nginx prefix and binary -------------------------------
REM The prefix is the folder holding conf\, logs\, and temp\ -- what -p is
REM given. Two layouts answer to that and both keep working: the current
REM one puts the versions inside a stable "nginx" folder with a "current"
REM junction naming the active build, and the original one made the
REM versioned folder the prefix itself. A folder qualifies by holding
REM conf\nginx.conf, which is what -c resolves to.
REM (`devcrate nginx migrate` converts the original layout to the current
REM one. The same resolution is mirrored in stop.bat and new-vhost.bat.)
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

if not defined NGX_EXE (
    echo   SKIPPED - no nginx.exe found under %ROOT%\nginx or %ROOT%\nginx-*
) else (
    REM Vhost confs use the prefix-relative "root projects/<domain>", which
    REM resolves through this junction. (PHP-CGI rejects paths containing "..",
    REM so the junction keeps SCRIPT_FILENAME dot-free.) Self-heals if missing.
    if not exist "%ROOT%\projects" mkdir "%ROOT%\projects"
    if not exist "%NGX_PREFIX%\projects" mklink /J "%NGX_PREFIX%\projects" "%ROOT%\projects" >nul
    start /B "Nginx" "%NGX_EXE%" -p "%NGX_PREFIX%" -c "conf/nginx.conf"
)

echo.
echo All services started.
echo   MariaDB  -^>  127.0.0.1:3306  (user: root, no password)
for /d %%D in ("%ROOT%\php\php-*") do call :show_php "%%~fD"
echo   RabbitMQ -^>  127.0.0.1:5672  (mgmt UI http://127.0.0.1:15672  guest/guest)
echo   Postgres -^>  127.0.0.1:5432
echo   Redis    -^>  127.0.0.1:6379
echo   Mailpit  -^>  SMTP 127.0.0.1:1025, inbox http://127.0.0.1:8025
echo   Nginx    -^>  :80 / :443
echo.
echo --- Vhosts (conf/sites/*.conf) ---
echo   api-qlearning.qhomeapps.test     -^>  PHP 8.2  (CodeIgniter 4, projects\api-qlearning.qhomeapps.id\public)
echo   asm.qhomemart.test               -^>  PHP 8.2  (CodeIgniter 3, projects\asm.qhomemart.cloud)
echo   hris.qhomedata.test              -^>  PHP 8.2  (CodeIgniter 3, projects\hris.qhomedata.id)
echo   ic-stokdigital.test              -^>  PHP 7.4  (CodeIgniter 3, projects\ic-stokdigital)
echo   qlearning.qhomeapps.test         -^>  PHP 8.2  (CodeIgniter 4, projects\qlearning.qhomeapps.id\public)
echo   supplier.qhomedata.id.test       -^>  PHP 7.4  (CodeIgniter 2, projects\supplier.qhomedata.id)
echo   supplier.qhomemart.cloud.test    -^>  PHP 7.4  (CodeIgniter 2, projects\supplier.qhomemart.cloud)
echo.
echo Run stop.bat to shut everything down cleanly.
goto :eof

REM ---------------------------------------------------------------------------
REM :start_php <folder> -- one PHP version as a FastCGI worker. A subroutine
REM rather than delayed expansion, so a "!" in the stack's path is harmless.
:start_php
set "PHP_DIR=%~1"
set "PHP_VER=%~nx1"
set "PHP_VER=%PHP_VER:php-=%"
set "PHP_DIGITS=%PHP_VER:.=%"
if not exist "%PHP_DIR%\php-cgi.exe" (
    echo   SKIPPED - %PHP_DIR%\php-cgi.exe not found
    goto :eof
)
set "PHP_FOUND=1"
echo   PHP %PHP_VER%  (port 90%PHP_DIGITS%)
start /B /D "%PHP_DIR%" "PHP%PHP_DIGITS%-CGI" "%PHP_DIR%\php-cgi.exe" -b 127.0.0.1:90%PHP_DIGITS%
goto :eof

:show_php
set "PHP_VER=%~nx1"
set "PHP_VER=%PHP_VER:php-=%"
set "PHP_DIGITS=%PHP_VER:.=%"
if exist "%~1\php-cgi.exe" echo   PHP %PHP_VER%  -^>  127.0.0.1:90%PHP_DIGITS%
goto :eof
