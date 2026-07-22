# Custom Local PHP Development Stack (Windows)

A portable, multi-PHP web stack that runs entirely out of `E:\dev` — no system-wide
installs. Nginx serves per-project `.test` vhosts over HTTPS, each pinned to a specific
PHP version via FastCGI, with MariaDB and RabbitMQ alongside.

> **This repo holds configuration only.** The runtime binaries, downloaded archives,
> TLS keys, runtime data, and the application code under `projects/` are intentionally
> **not** committed (see [`.gitignore`](.gitignore)). Follow the steps below to rebuild
> a working stack from a fresh clone.

## Documentation

Everything lives in [`docs/`](docs/index.md) ([index](docs/index.md)):

| Doc | What it covers |
| --- | --- |
| [Setup guide](docs/setup.md) | Linear walkthrough to get the stack running from a fresh clone. |
| [Architecture](docs/architecture.md) | Folder layout, request flow, ports, and design decisions. |
| [Installation](docs/installation.md) | Full rebuild reference: what to download and where it goes. |
| [PHP versions](docs/php-versions.md) | The `phpuse` CLI switcher, per-vhost FastCGI, and `php.ini`. |
| [Nginx & vhosts](docs/nginx-vhosts.md) | Adding sites, TLS certs with mkcert, per-framework web roots. |
| [Database](docs/database.md) | MariaDB usage: connecting, databases, imports, per-framework config. |
| [RabbitMQ](docs/rabbitmq.md) | Broker usage, management UI, PHP client, smoke test. |
| [Troubleshooting](docs/troubleshooting.md) | Common failures, known limitations, verification checklist. |

See [CHANGELOG.md](CHANGELOG.md) for release history.

## Stack

| Service    | Version   | Endpoint                          | Notes                          |
| ---------- | --------- | --------------------------------- | ------------------------------ |
| Nginx      | 1.31.1    | `:80` / `:443`                    | serves `conf/sites/*.conf`     |
| PHP (CLI)  | 7.4 / 8.2 / 8.5 | switchable via `phpuse`     | active version → `php/current` |
| PHP (FPM)  | per-vhost | `:9074` / `:9082` / …             | `php-cgi.exe` FastCGI workers  |
| MariaDB    | 12.3      | `127.0.0.1:3306`                  | user `root`, no password       |
| RabbitMQ   | 4.3.2     | `:5672` / mgmt `:15672`           | `guest` / `guest`              |

## Layout

```
E:\dev\
├─ start.bat / stop.bat     # bring the whole stack up / down
├─ new-vhost.bat            # scaffold a new nginx vhost + hosts entry
├─ phpuse.bat               # switch the global CLI PHP version
├─ docs\                    # full documentation (setup, architecture, ...)
├─ CHANGELOG.md             # release history
├─ nginx-1.31.1\conf\       # nginx.conf + sites\*.conf  (tracked)
├─ php\php74|php82|php85\    # each version's php.ini is tracked; binaries are not
├─ php\current              # junction → the active CLI PHP version (generated)
└─ projects\                # your app code — NOT tracked
```

## Rebuild from a fresh clone

Binaries aren't in the repo — download and extract them into place:

1. **Visual C++ Redistributable x64** (required, or PHP exits silently) —
   <https://aka.ms/vs/17/release/vc_redist.x64.exe>
2. **PHP** (Thread-Safe x64 ZIPs) from <https://windows.php.net/download/> →
   extract to `php\php74\`, `php\php82\`, `php\php85\`.
3. **Nginx 1.31.1** → extract so `nginx-1.31.1\nginx.exe` sits beside the tracked `conf\`.
4. **MariaDB 12.3** → `mariadb\`, **RabbitMQ 4.3.2 + Erlang** → `rabbitmq\` / `erlang\`.
5. **mkcert** → save as `mkcert.exe`, then generate the local TLS certs into
   `nginx-1.31.1\conf\certs\` (see [docs/setup.md](docs/setup.md); keys are never committed).
6. Recreate the CLI switcher junction and PATH — see **PHP version switching** below.
7. Drop your applications into `projects\`.

Then:

```bat
E:\dev\start.bat
```

## PHP version switching (CLI)

`php` on the CLI resolves through the `php\current` junction, so switching is instant:

```bat
phpuse            REM show active + installed versions
phpuse 85         REM switch global CLI PHP to 8.5
phpuse 82         REM ...or 8.2, 74, etc.
```

First-time setup on a new machine (creates the junction and puts it on PATH):

```bat
mklink /J E:\dev\php\current E:\dev\php\php85
```

Then add `E:\dev\php\current` and `E:\dev` to your **user** `PATH`, and open a new terminal.

## Adding a project vhost

```bat
new-vhost.bat
```

Follow the prompts (hostname, PHP version, docroot). It writes `conf/sites/<host>.conf`,
issues a mkcert cert, and adds the `.test` entry to your hosts file. Restart with
`stop.bat` then `start.bat`.
