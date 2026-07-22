# Devcrate

**A portable, multi-PHP development stack for Windows.** The whole stack lives in one
folder — the *stack root* — with no system-wide installs, and works from **any**
location: the scripts resolve their own path, and the nginx configs are relative.
Nginx serves per-project `.test` vhosts over HTTPS, each pinned to a specific PHP
version via FastCGI, with MariaDB and RabbitMQ alongside — a lightweight alternative
to XAMPP / Laragon / FlyEnv.

> Path examples in this repo use `C:\devcrate` as the stack root — substitute
> wherever you cloned it (`D:\tools\devcrate`, `E:\dev`, …).

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
| [Roadmap](docs/roadmap.md) | Planned development: the Rust/ratatui TUI, runtime installer, and CLI. |

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
C:\devcrate\                # ← the stack root (any folder works)
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
C:\devcrate\start.bat
```

## PHP version switching (CLI)

`php` on the CLI resolves through the `php\current` junction, so switching is instant:

```bat
phpuse            REM show active + installed versions
phpuse 85         REM switch global CLI PHP to 8.5
phpuse 82         REM ...or 8.2, 74, etc.
```

First-time setup on a new machine: run `phpuse 85` once (it creates the junction),
then add `<stack-root>\php\current` and `<stack-root>` to your **user** `PATH`
(e.g. `C:\devcrate\php\current` and `C:\devcrate`) and open a new terminal.

## Adding a project vhost

```bat
new-vhost.bat
```

Follow the prompts (hostname, PHP version, docroot). It writes `conf/sites/<host>.conf`,
issues a mkcert cert, and adds the `.test` entry to your hosts file. Restart with
`stop.bat` then `start.bat`.

## Roadmap

Devcrate is a prototype. The batch scripts are the starting point, not the destination:

1. **A Rust TUI** built with [ratatui](https://ratatui.rs/), shipped as a single
   `devcrate.exe` — dashboard, service control, log viewer, vhost and PHP-version
   management.
2. **Install runtimes from inside the program** — pick a version of PHP, Nginx,
   MariaDB, RabbitMQ, Erlang, or Composer and have Devcrate download, verify, and
   configure it, instead of fetching archives from vendor sites by hand.
3. **Run from any terminal** — one binary on `PATH` that works in cmd, PowerShell,
   Windows Terminal, and Git Bash, with scriptable subcommands (`devcrate start`,
   `devcrate php use 8.5`, …) behind the same executable as the TUI.

Details, constraints, and build order: [docs/roadmap.md](docs/roadmap.md).

## License

[MIT](LICENSE) © fadjardev
