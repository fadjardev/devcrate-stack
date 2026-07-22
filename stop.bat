@echo off
setlocal

REM Resolve the stack root from this script's own location (works anywhere).
set "ROOT=%~dp0"
set "ROOT=%ROOT:~0,-1%"

echo === Devcrate  -  PHP Development Stack (%ROOT%)  -  STOP ===
echo.

echo Gracefully stopping Nginx (waiting for active connections) ...
"%ROOT%\nginx-1.31.1\nginx.exe" -p "%ROOT%\nginx-1.31.1" -s quit
timeout /t 4 /nobreak > nul

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
