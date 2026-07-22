# Database - MariaDB 12.3

- **Location:** `C:\devcrate\mariadb\`
- **Config:** `C:\devcrate\mariadb\my.ini`
- **Data:** `C:\devcrate\mariadb\data\`
- **Error log:** `C:\devcrate\mariadb\mariadb_error.log`

MariaDB 12.3 (LTS) runs as a portable build, started/stopped by `start.bat` /
`stop.bat` alongside Nginx and PHP. It is a drop-in MySQL replacement - the
`mysqli` and `pdo_mysql` drivers (enabled in all three PHP versions) connect
unchanged.

## Connection defaults

| Setting | Value |
| --- | --- |
| Host | `127.0.0.1` |
| Port | `3306` |
| User | `root` |
| Password | *(none)* |

> Use `127.0.0.1`, not `localhost`. On Windows, `localhost` can make the client
> attempt a named pipe/socket instead of TCP.

## CLI client

```cmd
C:\devcrate\mariadb\bin\mariadb.exe -u root
```

## Create a database

```cmd
C:\devcrate\mariadb\bin\mariadb.exe -u root -e "CREATE DATABASE myapp CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci;"
```

## Import a SQL dump

```cmd
C:\devcrate\mariadb\bin\mariadb.exe -u root myapp < C:\path\to\dump.sql
```

## Set a root password (optional)

```cmd
C:\devcrate\mariadb\bin\mariadb-admin.exe -u root password "yourpassword"
```

Then update each app's DB config with the new password.

## Per-framework DB config

| Framework | File | Keys |
| --- | --- | --- |
| CI2 / CI3 | `application/config/database.php` | `hostname=127.0.0.1`, `username=root`, `password=`, `database=myapp`, `dbdriver=mysqli` |
| CI4 | `.env` | `database.default.hostname=127.0.0.1`, `database.default.username=root`, `database.default.password=`, `database.default.database=myapp` |
| Laravel | `.env` | `DB_CONNECTION=mysql`, `DB_HOST=127.0.0.1`, `DB_PORT=3306`, `DB_USERNAME=root`, `DB_PASSWORD=`, `DB_DATABASE=myapp` |

## Port 3306 conflict

If another MySQL/MariaDB (e.g. from FlyEnv/XAMPP) is running, it occupies port
3306 and `mariadbd.exe` fails to bind:

```
Can't start server: Bind on TCP/IP port ... 10048
```

Stop the other service, or change `port` in `mariadb\my.ini` to e.g. `3307`
(and update each app's DB config to match). See
[troubleshooting.md](troubleshooting.md#port-3306-already-in-use).
