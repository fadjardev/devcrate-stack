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

echo [1/6] MariaDB 12.3  (port 3306) ...
if exist "%ROOT%\mariadb\bin\mariadbd.exe" (
    start /B "MariaDB" "%ROOT%\mariadb\bin\mariadbd.exe" --defaults-file="%ROOT%\mariadb\my.ini"
) else ( echo   SKIPPED - %ROOT%\mariadb\bin\mariadbd.exe not found )

echo [2/6] PHP 7.4  (port 9074) ...
if exist "%ROOT%\php\php-7.4\php-cgi.exe" (
    start /B /D "%ROOT%\php\php-7.4" "PHP74-CGI" "%ROOT%\php\php-7.4\php-cgi.exe" -b 127.0.0.1:9074
) else ( echo   SKIPPED - %ROOT%\php\php-7.4\php-cgi.exe not found )

echo [3/6] PHP 8.2  (port 9082) ...
if exist "%ROOT%\php\php-8.2\php-cgi.exe" (
    start /B /D "%ROOT%\php\php-8.2" "PHP82-CGI" "%ROOT%\php\php-8.2\php-cgi.exe" -b 127.0.0.1:9082
) else ( echo   SKIPPED - %ROOT%\php\php-8.2\php-cgi.exe not found )

echo [4/6] PHP 8.5  (port 9085) ...
if exist "%ROOT%\php\php-8.5\php-cgi.exe" (
    start /B /D "%ROOT%\php\php-8.5" "PHP85-CGI" "%ROOT%\php\php-8.5\php-cgi.exe" -b 127.0.0.1:9085
) else ( echo   SKIPPED - %ROOT%\php\php-8.5\php-cgi.exe not found ^(download PHP 8.5 to enable^) )

echo [5/6] RabbitMQ 4.3.2  (ports 5672 / 15672) ...
if exist "%ROOT%\rabbitmq\sbin\rabbitmq-server.bat" (
    REM -detached: run the broker as a background Erlang node. Do NOT redirect
    REM stdout/stderr to a file -- OTP's terminal driver crashes with "nouser"
    REM when its console is a non-tty. Logs go to %RABBITMQ_BASE%\log\rabbit@HOST.log
    start /B "RabbitMQ" cmd /c ""%ROOT%\rabbitmq\sbin\rabbitmq-server.bat" -detached"
) else ( echo   SKIPPED - %ROOT%\rabbitmq\sbin\rabbitmq-server.bat not found )

REM Give php-cgi a moment to bind before nginx starts proxying
timeout /t 2 /nobreak > nul

echo [6/6] Nginx ...
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
echo   PHP 7.4  -^>  127.0.0.1:9074
echo   PHP 8.2  -^>  127.0.0.1:9082
echo   PHP 8.5  -^>  127.0.0.1:9085
echo   RabbitMQ -^>  127.0.0.1:5672  (mgmt UI http://127.0.0.1:15672  guest/guest)
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
