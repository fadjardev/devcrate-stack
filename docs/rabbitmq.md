# Message broker - RabbitMQ 4.3.2

- **Broker:** `C:\devcrate\rabbitmq\`
- **Erlang:** `C:\devcrate\erlang\` (OTP 27)
- **Data / logs / config:** `C:\devcrate\rabbitmq\data\`
- **Log file:** `C:\devcrate\rabbitmq\data\log\rabbit@<HOSTNAME>.log`

RabbitMQ 4.3.2 runs as a portable build alongside Nginx/PHP/MariaDB, started and
stopped by `start.bat` / `stop.bat`. It is written in Erlang, so Erlang/OTP 27 is
a hard prerequisite (RabbitMQ 4.3.x supports OTP 26-27, not 28+).

Everything is kept off `C:` via environment variables that `start.bat` sets
before launching the broker:

```bat
set "ERLANG_HOME=C:\devcrate\erlang"
set "RABBITMQ_BASE=C:\devcrate\rabbitmq\data"
set "PATH=C:\devcrate\erlang\bin;%PATH%"
```

`RABBITMQ_BASE` is what relocates the node's data, logs, config, and the
`enabled_plugins` file onto `E:`. Without it, RabbitMQ defaults to
`%APPDATA%\RabbitMQ` on `C:`.

## Connection defaults

| Setting | Value |
| --- | --- |
| Host | `127.0.0.1` |
| AMQP port | `5672` |
| Management UI | `http://127.0.0.1:15672` |
| User / Pass | `guest` / `guest` (localhost only) |
| Node name | `rabbit@<HOSTNAME>` |

The default `guest` user can only connect over loopback (127.0.0.1), which is
fine for local dev. To connect from another host, create a real user with
`rabbitmqctl add_user` / `set_permissions`.

## Management UI

The `rabbitmq_management` plugin is already enabled (via
`rabbitmq\data\enabled_plugins`). After `start.bat`, open
`http://127.0.0.1:15672` and log in with `guest` / `guest`.

## Manual control

The Erlang env must be set first, so the easiest place to run these is the
window `start.bat` was launched from:

```cmd
REM start in the foreground (Ctrl+C to stop)
C:\devcrate\rabbitmq\sbin\rabbitmq-server.bat

REM status / stop / list queues
C:\devcrate\rabbitmq\sbin\rabbitmqctl.bat status
C:\devcrate\rabbitmq\sbin\rabbitmqctl.bat stop
C:\devcrate\rabbitmq\sbin\rabbitmqctl.bat list_queues
```

## PHP client (php-amqplib)

Pure-PHP client, no PECL extension required:

```cmd
composer require php-amqplib/php-amqplib
```

```php
use PhpAmqpLib\Connection\AMQPStreamConnection;

$conn = new AMQPStreamConnection('127.0.0.1', 5672, 'guest', 'guest');
```

- **`ext-sockets` is required** by php-amqplib and is enabled in every PHP
  version's `php.ini` (`extension=sockets`); `php_sockets.dll` ships with the
  builds.
- **RabbitMQ 4.x gotcha:** transient (non-durable) queues are refused by default.
  Declare queues as durable:
  `queue_declare($q, false, true, false, false)`.

## Smoke test

A standalone publish -> consume round-trip lives at
`C:\devcrate\tools\rabbitmq-smoketest\`. With the broker running:

```cmd
C:\devcrate\php\php-8.2\php.exe C:\devcrate\tools\rabbitmq-smoketest\test.php
```

Expected output ends with `RESULT: OK - round trip succeeded`.

## epmd lingers after shutdown

The Erlang Port Mapper Daemon (`epmd.exe`) stays resident even after
`rabbitmqctl stop`. `stop.bat` force-kills `erl.exe` and `epmd.exe` as a
fallback so the node restarts cleanly next time. See
[troubleshooting.md](troubleshooting.md#rabbitmq-wont-restart-cleanly).
