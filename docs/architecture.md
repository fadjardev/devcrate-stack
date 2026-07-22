# Architecture

## What this is

Devcrate is a portable, multi-PHP web stack that runs entirely from `E:\dev` on
Windows, with nothing installed system-wide. It reproduces a production-like LEMP setup
(Linux -> Windows here) so that several legacy and modern PHP applications, each
pinned to a different PHP version, can run side by side on one machine.

The stack deliberately keeps all binaries, data, and caches off the `C:` drive.
Each runtime is a portable extract under `E:\dev`, and the orchestration is a
pair of batch files (`start.bat` / `stop.bat`).

## Folder layout

```
E:\dev\
|- start.bat / stop.bat        Bring the whole stack up / down
|- new-vhost.bat               Scaffold a new nginx vhost (+ hosts hint)
|- phpuse.bat                  Switch the global CLI PHP version
|- README.md                   Overview + documentation index
|- docs\                       Documentation (setup, architecture, per-service, ...)
|
|- php\
|  |- php74\  php82\  php85\    Portable PHP builds (php.ini is versioned; binaries are not)
|  |- current                  Junction -> the active CLI PHP version (generated)
|
|- nginx-1.31.1\
|  |- nginx.exe                Binary (not versioned)
|  |- conf\
|  |  |- nginx.conf            Main config (versioned)
|  |  |- sites\*.conf          Per-project vhosts (versioned)
|  |  |- certs\                mkcert TLS certs + keys (NOT versioned)
|  |- logs\                    Access / error logs (not versioned)
|
|- mariadb\                    Portable MariaDB (binaries + data, not versioned)
|- rabbitmq\ + erlang\         Portable RabbitMQ broker + Erlang runtime (not versioned)
|- composer\                   Composer home + cache, kept off C: (not versioned)
|- tools\rabbitmq-smoketest\   Standalone AMQP publish/consume test
|- projects\                   Application code, one folder per site (NOT versioned)
```

## Request flow

```
Browser
  |  https://asm.qhomemart.test
  v
Windows hosts file  (127.0.0.1  asm.qhomemart.test)
  v
Nginx :443  (matches server_name in conf\sites\asm.qhomemart.test.conf)
  |  terminates TLS with the matching mkcert wildcard cert
  |  location ~ \.php$  ->  fastcgi_pass 127.0.0.1:<port>
  v
PHP FastCGI worker  (php-cgi.exe bound to 127.0.0.1:9074 / 9082 / 9085)
  |  runs the app; mysqli / pdo_mysql
  v
MariaDB 127.0.0.1:3306        (and optionally RabbitMQ 127.0.0.1:5672)
```

Each vhost hard-codes which PHP FastCGI port it proxies to, so the PHP version
is decided per site by the `fastcgi_pass` line - independent of the CLI version
selected by `phpuse`.

## Ports

| Service | Address | Notes |
| --- | --- | --- |
| MariaDB | `127.0.0.1:3306` | user `root`, no password |
| PHP 7.4 FastCGI | `127.0.0.1:9074` | `php-cgi.exe` |
| PHP 8.2 FastCGI | `127.0.0.1:9082` | `php-cgi.exe` |
| PHP 8.5 FastCGI | `127.0.0.1:9085` | `php-cgi.exe` |
| RabbitMQ AMQP | `127.0.0.1:5672` | broker |
| RabbitMQ mgmt UI | `127.0.0.1:15672` | `guest` / `guest` |
| Nginx HTTP | `0.0.0.0:80` | redirects to HTTPS |
| Nginx HTTPS | `0.0.0.0:443` | TLS via mkcert |

The FastCGI port convention is `90` + the PHP version digits: `php74 -> 9074`,
`php82 -> 9082`, `php85 -> 9085`.

## Two ways PHP version is selected

1. **Web (per site):** the `fastcgi_pass` port in each vhost. Changing a site's
   PHP version means pointing its `fastcgi_pass` at a different port and
   restarting. See [nginx-vhosts.md](nginx-vhosts.md).
2. **CLI (global):** the `php\current` junction, repointed by `phpuse`. This is
   what `php`, `composer`, and `laravel` on the command line resolve to. See
   [php-versions.md](php-versions.md).

## Design decisions

- **Everything off C:.** Portable extracts plus `COMPOSER_HOME`,
  `COMPOSER_CACHE_DIR`, `ERLANG_HOME`, and `RABBITMQ_BASE` all point into
  `E:\dev`. Nothing writes to `%APPDATA%` on `C:`.
- **Thread-safe (TS) PHP builds.** `php-cgi.exe` is run as a long-lived FastCGI
  listener with multiple child workers (`PHP_FCGI_CHILDREN=4`), so the ZTS
  (thread-safe) Windows builds are used.
- **mkcert wildcard certs.** One wildcard per domain group (`*.qhomeapps.test`,
  etc.) rather than per-host certs. X.509 does not allow multi-level wildcards,
  so each group gets its own. See [nginx-vhosts.md](nginx-vhosts.md).
- **Config is the only thing versioned.** Binaries are downloadable and large;
  the repo tracks the scripts, `nginx.conf`, `sites\*.conf`, and each
  `php.ini`. TLS private keys and `projects/` are never committed.
