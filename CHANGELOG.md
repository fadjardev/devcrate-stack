# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed

- **The stack is now location-independent** - it runs from any folder (the
  *stack root*), not just `E:\dev`:
  - `start.bat`, `stop.bat`, `phpuse.bat`, and `new-vhost.bat` resolve the
    stack root from their own location (`%~dp0`) instead of hard-coding it.
  - Vhost confs use nginx-prefix-relative paths: `root projects/<domain>`
    (through a `nginx-1.31.1\projects -> ..\projects` junction that
    `start.bat`/`new-vhost.bat` auto-create), `certs/...` (conf-relative), and
    `logs/...`. The junction exists because PHP-CGI on Windows rejects `..` in
    `SCRIPT_FILENAME` ("No input file specified"), so roots must stay dot-free.
  - `php.ini` for 7.4/8.2 now uses relative `extension_dir = "ext"` and
    `error_log = "php_errors.log"`; `start.bat` launches each `php-cgi.exe`
    with its own folder as working directory so relative paths resolve there.
  - Documentation now uses `C:\devcrate` as a stand-in example root and
    describes the stack-root convention; first-time CLI setup is `phpuse 85`
    (which creates the `php\current` junction) instead of a manual `mklink`.
  - The only machine-specific absolute paths left are outside the repo: user
    `PATH` entries and the `php\current` junction target.

### Added

- **The `devcrate` binary** (`devcrate/`) - a Rust CLI, and the first step of
  the roadmap's sequencing item 1. It is **read-only**: it reports on the stack
  and changes nothing.
  - `devcrate status` (`--json` for scripts) - per service: installed or not,
    running or not, which ports answer, and the PIDs. Distinguishes a service
    that is *running* from a port held by *something else*, by matching the
    running process's executable path against the stack root rather than its
    image name.
  - `devcrate config show` / `config path` - prints the resolved configuration
    as TOML, so the values currently discovered from the folder layout can be
    pinned into a `devcrate.toml`.
  - `devcrate php list` - installed versions, their FastCGI ports, and which
    one `php\current` resolves to. `devcrate site list` - the vhosts with their
    `root` and FastCGI port.
  - `start`, `stop`, `restart`, `php use`, `site add`/`remove`, and `install`
    are declared so the command surface is settled, but exit 3 and name the
    batch script that does the job today.
  - Stack root resolved from `--root`, then `DEVCRATE_HOME`, then the
    executable's folder, then the working directory - the last two searching
    upward, so it works from anywhere inside the tree.
  - `devcrate.toml` is optional and so is every key in it; anything missing is
    discovered from the layout (the `nginx-*` directory, one PHP entry per
    `php\php*` folder, FastCGI port `90` + version digits).
  - Documented in `docs/cli.md`, linked from the README and the docs index.
- **MIT license** (`LICENSE`), with a License section in the README.
- **`docs/roadmap.md`** - planned development: a Rust/ratatui TUI shipped as a
  single executable, an in-program downloader/installer for PHP, Nginx,
  MariaDB, RabbitMQ, Erlang, and Composer, a cross-terminal binary with
  scriptable subcommands, one-step setup for an existing project (vhost +
  `hosts` entry + mkcert TLS), and Node.js/Bun as managed runtimes. Summarized
  in the README and linked from the docs index.

### Fixed

- README's "Adding a project vhost" section described behaviour `new-vhost.bat`
  does not have: it takes `<domain> <phpversion>` as arguments rather than
  prompting, and it only *prints* the hosts-file line instead of adding it -
  nor does it issue a certificate (it reuses the existing `*.test` wildcard).
  Rewritten to match the script, with the two manual steps called out.

## [0.1.0] - 2026-07-22

First tagged prototype of **Devcrate**, a portable, multi-PHP development stack
for Windows - a self-contained alternative to XAMPP / Laragon / FlyEnv that runs
entirely from `E:\dev`.

### Added

- **Orchestration scripts**
  - `start.bat` / `stop.bat` - bring the whole stack (MariaDB, PHP FastCGI,
    RabbitMQ, Nginx) up and down cleanly.
  - `new-vhost.bat` - scaffold a project vhost (`public/` web root, TLS cert,
    nginx reload) for `php74`, `php82`, or `php85`.
  - `phpuse.bat` - switch the global CLI PHP version instantly via a
    `php\current` junction on PATH.
- **Services**
  - Nginx 1.31.1 serving per-project `.test` vhosts over HTTPS.
  - PHP 7.4 / 8.2 / 8.5 as FastCGI listeners on ports 9074 / 9082 / 9085.
  - MariaDB 12.3 on `127.0.0.1:3306`.
  - RabbitMQ 4.3.2 (Erlang/OTP 27) with the management UI on `:15672`.
- **PHP 8.5 (8.5.8)** installed and wired into both the CLI switcher and the web
  stack (FastCGI on port 9085).
- **CLI PHP version switcher** - `phpuse` plus the `php\current` junction and a
  user PATH entry, so `php`, `composer`, and `laravel` resolve to the selected
  version.
- **TLS** via mkcert with one wildcard certificate per domain group.
- **Documentation** under `docs/`: setup, architecture, installation,
  php-versions, nginx-vhosts, database, rabbitmq, and troubleshooting, indexed
  from the root README.
- **Repository hygiene**
  - `.gitignore` tracking only scripts and service configs; excluding binaries,
    runtime data, downloaded archives, TLS private keys, and `projects/`.
  - `.gitattributes` normalizing line endings (`.bat`/`.cmd`/`.ps1` as CRLF,
    configs/docs as LF, binaries flagged).

### Changed

- `start.bat` and `new-vhost.bat` now serve/scaffold **PHP 8.5** (port 9085) in
  place of the never-installed PHP 8.3 slot.
- Moved the setup guide to `docs/setup.md` and rewrote it: fixed double-encoded
  UTF-8 mojibake, corrected all PHP 8.3 / port 9083 references to 8.5 / 9085, and
  documented the `phpuse` switcher.

### Fixed

- Double-encoded UTF-8 characters in the setup guide and the `stop.bat` banner.

[Unreleased]: https://github.com/fadjardev/devcrate-stack/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/fadjardev/devcrate-stack/releases/tag/v0.1.0
