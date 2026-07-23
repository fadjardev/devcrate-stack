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
| [The `devcrate` binary](docs/cli.md) | The Rust CLI: building it, stack-root resolution, `devcrate.toml`, which subcommands work today. |
| [The dashboard](docs/tui.md) | The interactive terminal UI: panes, keys, crash detection, how it stays responsive. |
| [Roadmap](docs/roadmap.md) | Planned development: the Rust/ratatui TUI, runtime installer, CLI, one-step project setup, Node/Bun. |

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
├─ devcrate\                # Rust sources for the devcrate CLI
├─ docs\                    # full documentation (setup, architecture, ...)
├─ CHANGELOG.md             # release history
├─ nginx\                   # the nginx prefix — it never moves
│  ├─ conf\                 #   nginx.conf + sites\*.conf + certs\  (conf tracked)
│  ├─ nginx-1.31.1\         #   one folder per nginx build (not tracked)
│  └─ current               #   junction → the active nginx build (generated)
├─ php\php-7.4|php-8.2|php-8.5\  # each version's php.ini is tracked; binaries are not
├─ php\current              # junction → the active CLI PHP version (generated)
└─ projects\                # your app code — NOT tracked
```

## Rebuild from a fresh clone

Binaries aren't in the repo — download and extract them into place:

1. **Visual C++ Redistributable x64** (required, or PHP exits silently) —
   <https://aka.ms/vs/17/release/vc_redist.x64.exe>
2. **PHP** (Thread-Safe x64 ZIPs) from <https://windows.php.net/download/> →
   extract to `php\php-7.4\`, `php\php-8.2\`, `php\php-8.5\` — or, once the
   `devcrate` binary is built, `devcrate install php 8.5` downloads, verifies,
   and installs a version itself.
3. **Nginx** → extract into `nginx\`, so the build lands at
   `nginx\nginx-1.31.1\nginx.exe` beside the tracked `nginx\conf\`, then run
   `devcrate nginx use 1.31.1` — or let `devcrate install nginx 1.31.3` do the
   download, the unpack, and the junction.
4. **MariaDB 12.3** → `mariadb\`, **RabbitMQ 4.3.2 + Erlang** → `rabbitmq\` / `erlang\`.
5. **mkcert** → save as `mkcert.exe`, then generate the local TLS certs into
   `nginx\conf\certs\` (see [docs/setup.md](docs/setup.md); keys are never committed).
   They live under the prefix, not under a version, so switching nginx builds
   leaves them alone.
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
phpuse 8.5        REM switch global CLI PHP to 8.5
phpuse 7.4        REM ...or 8.2, 7.4, etc.
```

Each version lives in its own `php\php-<X.Y>\` folder, and the version can be
spelled `8.5`, `85`, or `php-8.5` — everything matches on the digits.

First-time setup on a new machine: run `phpuse 8.5` once (it creates the junction),
then add `<stack-root>\php\current` and `<stack-root>` to your **user** `PATH`
(e.g. `C:\devcrate\php\current` and `C:\devcrate`) and open a new terminal.

## Adding a project vhost

```bat
new-vhost.bat myapp.test 8.5
```

It scaffolds `projects\myapp.test\public\`, writes `conf/sites/myapp.test.conf`
(reusing the `*.test` wildcard cert), and reloads nginx. Two steps are still
manual today — adding `127.0.0.1  myapp.test` to your hosts file as
Administrator (the script prints the line), and issuing a new mkcert wildcard
for third-level domains such as `api.mygroup.test`. Automating both is
[roadmap item 4](docs/roadmap.md). See [Nginx & vhosts](docs/nginx-vhosts.md)
for the details.

## Roadmap

Devcrate is a prototype. The batch scripts are the starting point, not the destination:

1. ✅ **A Rust TUI** built with [ratatui](https://ratatui.rs/), shipped as a single
   `devcrate.exe` — dashboard, service control, log viewer, vhost and PHP-version
   management. **Built** — see [docs/tui.md](docs/tui.md).
2. 🚧 **Install runtimes from inside the program** — pick a version of PHP, Nginx,
   MariaDB, RabbitMQ, Erlang, or Composer and have Devcrate download, verify, and
   configure it, instead of fetching archives from vendor sites by hand.
   **Built for PHP and Nginx**: `devcrate install php 8.4` downloads the
   release from windows.php.net, verifies its sha256 against the vendor's own
   feed, and installs it — `php.ini` generated, non-thread-safe builds refused.
   `devcrate install nginx 1.31.3` does the same from nginx.org, into the
   prefix beside any build already there — though nginx publishes no checksum,
   which the command says rather than glosses over. `--from <zip>` installs
   either offline. MariaDB, RabbitMQ, Erlang, and Composer are still to come.
3. ✅ **Run from any terminal** — one binary on `PATH` that works in cmd, PowerShell,
   Windows Terminal, and Git Bash, with scriptable subcommands (`devcrate start`,
   `devcrate php use 8.5`, …) behind the same executable as the TUI.
   **Built** — see [docs/cli.md](docs/cli.md).
4. **Open an existing project in one step** — point Devcrate at a folder you already
   have and get the vhost, the `hosts` entry, and a trusted local TLS certificate
   issued with mkcert, without editing any of them by hand.
5. **Node.js and Bun as managed runtimes** — installed and version-switched the same
   way PHP is, so a project's front-end toolchain lives inside the stack root too.

Details, constraints, and build order: [docs/roadmap.md](docs/roadmap.md).

**Where it is:** items 1 and 3 are built. The Rust crate under
[`devcrate/`](devcrate) is one executable with two front ends over one core —
run `devcrate` for the ratatui dashboard, or a subcommand for scriptable output:

```bat
devcrate                          REM the dashboard
devcrate status --json            REM ...or one thing, for a script
devcrate start / stop / restart
devcrate php use 8.5
devcrate nginx list / use / migrate
devcrate site add / set-php / remove
devcrate install php 8.4 / install nginx 1.31.3
```

Every batch script above has an equivalent in both. They act only on the
processes belonging to their own stack root, name the process holding a port
they wanted, and test the nginx configuration before reloading it — none of
which the scripts can do. The dashboard adds the one thing a command cannot: it
stays resident, so a service that dies on its own reads `crashed` rather than
`stopped`. The scripts keep working. `devcrate install php 8.4` and `devcrate
install nginx 1.31.3` download and install a runtime (item 2); MariaDB,
RabbitMQ, Erlang, Composer, the hosts file, and mkcert (item 4) are still
manual. See [docs/cli.md](docs/cli.md) and [docs/tui.md](docs/tui.md).

## License

[MIT](LICENSE) © fadjardev
