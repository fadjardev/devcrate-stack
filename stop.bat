@echo off
echo === E:\dev  PHP Development Stack â€“ STOP ===
echo.

echo Gracefully stopping Nginx (waiting for active connections) ...
"E:\dev\nginx-1.31.1\nginx.exe" -p "E:\dev\nginx-1.31.1" -s quit
timeout /t 4 /nobreak > nul

echo Terminating PHP-CGI processes ...
taskkill /F /IM php-cgi.exe /T 2>nul

echo Gracefully stopping RabbitMQ ...
set "ERLANG_HOME=E:\dev\erlang"
set "RABBITMQ_BASE=E:\dev\rabbitmq\data"
set "PATH=E:\dev\erlang\bin;%PATH%"
if exist "E:\dev\rabbitmq\sbin\rabbitmqctl.bat" (
    call "E:\dev\rabbitmq\sbin\rabbitmqctl.bat" stop 2>nul
    timeout /t 3 /nobreak > nul
)
taskkill /F /IM erl.exe /T 2>nul
taskkill /F /IM epmd.exe /T 2>nul

echo Gracefully stopping MariaDB ...
if exist "E:\dev\mariadb\bin\mariadb-admin.exe" (
    "E:\dev\mariadb\bin\mariadb-admin.exe" --defaults-file=E:\dev\mariadb\my.ini -u root shutdown 2>nul
    timeout /t 3 /nobreak > nul
)
taskkill /F /IM mariadbd.exe /T 2>nul

echo.
echo All services stopped.
