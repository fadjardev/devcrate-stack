# Roadmap

Where Devcrate is going. Today it is a set of batch scripts plus configuration
that you drive by hand; the goal is a single executable that manages the whole
stack for you.

**Status legend:** `planned` - agreed, not started. `in progress` - being built.
`done` - shipped, see [CHANGELOG.md](../CHANGELOG.md).

| # | Item | Status |
| --- | --- | --- |
| 1 | Rust TUI (`ratatui`), shipped as a single executable | planned |
| 2 | Built-in runtime downloader / installer with version selection | planned |
| 3 | Runs from any terminal, scriptable as well as interactive | planned |

---

## 1. Rust TUI built with ratatui

Replace the batch-file workflow with one interactive terminal UI, written in
Rust using [ratatui](https://ratatui.rs/) and
[crossterm](https://docs.rs/crossterm/), compiled to a single
`devcrate.exe` with no runtime dependency.

**What it should do**

- **Dashboard** - live status of every service (Nginx, each PHP FastCGI
  listener, MariaDB, RabbitMQ): running / stopped, PID, bound port, uptime.
- **Start / stop / restart** - per service and for the whole stack, replacing
  `start.bat` and `stop.bat` while keeping the same graceful-shutdown order
  (nginx `-s quit` -> PHP -> RabbitMQ via `rabbitmqctl stop` -> MariaDB via
  `mariadb-admin shutdown`).
- **Log viewer** - tail `nginx-1.31.1\logs\*.log`, the MariaDB error log, and
  the RabbitMQ node log inside the TUI, with per-site filtering.
- **Vhost manager** - list `conf/sites/*.conf`, create a site (what
  `new-vhost.bat` does now: scaffold the web root, write the vhost, issue the
  mkcert certificate, print/patch the hosts entry), edit its PHP version, and
  reload nginx.
- **PHP switcher** - what `phpuse.bat` does now: repoint the `php\current`
  junction and show which version the CLI resolves to.
- **Port conflict detection** - before starting, check that 80, 443, 3306,
  5672, 15672, and each `90xx` FastCGI port are free, and say which process
  holds one when it isn't.

**Design constraints**

- Process supervision, not just spawning: track child PIDs so the TUI can
  report a crashed service instead of silently showing it as "up".
- The TUI is a controller for the existing layout, not a new one - it keeps
  reading and writing the same `nginx.conf`, `sites\*.conf`, and per-version
  `php.ini` files documented in [architecture.md](architecture.md), so a
  hand-edited config never gets clobbered.
- Config for the tool itself lives in one versionable file
  (`devcrate.toml`) holding the stack root, installed runtime versions, and
  port assignments. (The batch scripts already resolve the stack root from
  their own location; `devcrate.toml` carries that further so the tool can
  manage a stack it doesn't live inside.)

**Rough crate list:** `ratatui`, `crossterm`, `tokio`, `serde` + `toml`,
`sysinfo` (process/port inspection), `tracing`.

---

## 2. Download and install runtimes from inside the program

Right now every runtime is fetched manually from a website and extracted by
hand ([installation.md](installation.md)). Devcrate should do that itself.

**Managed runtimes:** PHP, Nginx, MariaDB, RabbitMQ, Erlang/OTP, Composer.

**What it should do**

- **Version catalogue** - list the versions available for each runtime,
  resolved from the upstream sources rather than a list baked into the binary:

  | Runtime | Source |
  | --- | --- |
  | PHP | `windows.php.net/downloads/releases/` (+ `/archives/` for EOL builds) |
  | Nginx | `nginx.org/en/download.html` (Windows zips) |
  | MariaDB | MariaDB downloads REST API |
  | RabbitMQ | GitHub releases (`rabbitmq/rabbitmq-server`) |
  | Erlang/OTP | GitHub releases (`erlang/otp`) - Windows installer / portable |
  | Composer | `getcomposer.org/download/` (+ `installer.sig`) |

- **Install** - pick a version, download with a progress bar, verify the
  checksum/signature the vendor publishes, extract into the standard layout
  (`php\php85\`, `nginx-1.31.1\`, `mariadb\`, ...), and generate the
  first-run config (a `php.ini` seeded from `php.ini-development` with the
  extension set Devcrate expects; `my.ini`; the FastCGI port).
- **Multiple versions side by side** for PHP specifically - installing 8.5
  must not disturb 7.4 or 8.2, and it should register the new FastCGI port
  (`90` + version digits) and offer it in the vhost editor and `phpuse`.
- **Uninstall / prune** - remove a version and warn first if any vhost still
  points at its FastCGI port.
- **Preflight checks** - detect the missing Visual C++ redistributable, which
  is the single most common cause of `php-cgi.exe` exiting silently, and link
  the installer instead of letting the stack fail mysteriously.

**Constraints**

- Always Thread-Safe (TS) x64 PHP builds - the stack runs `php-cgi.exe` as a
  long-lived FastCGI listener with `PHP_FCGI_CHILDREN`, see
  [php-versions.md](php-versions.md).
- Downloads are verified before extraction; nothing is executed straight from
  the network, and no runtime is installed outside the stack root.
- Fully offline-capable fallback: an already-downloaded archive can be pointed
  at and installed without network access.
- Erlang ships as an installer rather than a plain zip, so it needs a
  silent-install path (or a portable extraction) that still keeps
  `ERLANG_HOME` inside the stack root.

---

## 3. Run from any terminal

The tool should not be tied to one shell or to being launched by
double-clicking a `.bat` file.

**What it should do**

- **One binary on PATH** - `devcrate` works identically from `cmd.exe`,
  PowerShell, Windows Terminal, Git Bash, and an IDE's integrated terminal.
- **Both modes from the same executable:**

  ```
  devcrate                    # launch the TUI
  devcrate start [service]    # non-interactive, for scripts and CI
  devcrate stop [service]
  devcrate status             # machine-readable with --json
  devcrate php use 8.5
  devcrate install php 8.5
  devcrate site add myapp.test --php 8.5
  ```

  The subcommands are what makes it usable from a script, a task runner, or a
  keybinding - the TUI is a front end over the same commands.
- **Terminal-capability aware** - detect ANSI/truecolor support and degrade
  cleanly on terminals that lack it; never assume a specific width; leave the
  terminal in a sane state on exit or panic (restore the alternate screen and
  cooked mode).
- **No hard-coded paths** - resolve the stack root from the executable
  location, a `DEVCRATE_HOME` environment variable, or `devcrate.toml`. (The
  batch scripts already do the `%~dp0` version of this; the binary keeps that
  property.)
- **Correct exit codes** so `devcrate start && ...` behaves in a script.

**Beyond Windows.** Nothing above is Windows-specific except the runtime
archives and the `php\current` junction. Once the path handling is abstracted,
a Linux/macOS build (symlink instead of junction, distro packages or upstream
tarballs instead of Windows zips) becomes a realistic follow-up rather than a
rewrite - so platform-specific code should stay behind a thin abstraction from
the start.

---

## Sequencing

1. Rust project skeleton, `devcrate.toml` config model, path/stack-root
   resolution, and the CLI subcommand surface (item 3's plumbing).
2. Port the existing scripts behind those subcommands: `start`, `stop`,
   `status`, `php use`, `site add`. At this point the batch files still work
   and can be retired one at a time.
3. The ratatui dashboard on top of that core (item 1).
4. The runtime installer, starting with PHP - it has the most versions and the
   most benefit - then Nginx, Composer, MariaDB, and RabbitMQ/Erlang (item 2).

The batch scripts stay in the repo and keep working until the equivalent
subcommand is shipped and documented.
