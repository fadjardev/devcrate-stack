# Download and install runtimes from inside the program #2

## Summary

Devcrate should fetch, verify, extract, and configure its own runtimes — PHP, Nginx,
MariaDB, RabbitMQ, Erlang/OTP, and Composer — instead of requiring you to download
archives from vendor sites and extract them by hand as
[docs/installation.md](docs/installation.md) describes today.

Roadmap item 2 of 3 — see [docs/roadmap.md](docs/roadmap.md).

## Motivation

Rebuilding the stack from a fresh clone is currently a manual scavenger hunt across
six websites, and it is easy to grab the wrong PHP build (NTS instead of TS, or the
wrong VC++ toolchain) and then spend an afternoon on a `php-cgi.exe` that exits with
no output. The program knows exactly what it needs — it should just get it.

## Scope

### Version catalogue

- [ ] List available versions per runtime, resolved from upstream at runtime rather
  than from a list baked into the binary
- [ ] Cache the catalogue so the UI stays responsive offline

| Runtime    | Source                                                                |
| ---------- | --------------------------------------------------------------------- |
| PHP        | `windows.php.net/downloads/releases/` (+ `/archives/` for EOL builds) |
| Nginx      | `nginx.org/en/download.html` (Windows zips)                           |
| MariaDB    | MariaDB downloads REST API                                            |
| RabbitMQ   | GitHub releases (`rabbitmq/rabbitmq-server`)                          |
| Erlang/OTP | GitHub releases (`erlang/otp`) — Windows installer / portable         |
| Composer   | `getcomposer.org/download/` (+ `installer.sig`)                       |

### Install

- [ ] Pick a version and download it with a progress bar
- [ ] Verify the checksum/signature the vendor publishes **before** extracting
- [ ] Extract into the standard layout (`php\php-8.5\`, `nginx-1.31.1\`, `mariadb\`, …)
- [ ] Generate first-run config: a `php.ini` seeded from `php.ini-development` with
  the extension set Devcrate expects (curl, exif, fileinfo, gd, intl, mbstring,
  openssl, pdo_mysql, pdo_sqlite, sodium, sqlite3, zip), `my.ini`, and the
  FastCGI port

### Multiple PHP versions side by side

- [ ] Installing 8.5 must not disturb 7.4 or 8.2
- [ ] Register the new FastCGI port (`90` + version digits) and offer the version in
  the vhost editor and in `php use`

### Uninstall

- [ ] Remove a version
- [ ] Refuse (or warn loudly) if a vhost still points at its FastCGI port

### Preflight

- [ ] Detect a missing Visual C++ redistributable — the single most common cause of
  `php-cgi.exe` exiting silently — and link the installer instead of letting the
  stack fail mysteriously

## Constraints

- **Thread-Safe (TS) x64 PHP builds only.** The stack runs `php-cgi.exe` as a
  long-lived FastCGI listener with `PHP_FCGI_CHILDREN`; see
  [docs/php-versions.md](docs/php-versions.md).
- **Verify before extract.** Nothing is executed straight from the network, and no
  runtime is ever installed outside the stack root.
- **Offline fallback.** An already-downloaded archive can be pointed at and installed
  with no network access.
- **Erlang ships as an installer, not a plain zip.** Needs a silent-install path (or
  a portable extraction) that still keeps `ERLANG_HOME` inside the stack root.

## Open questions

- Where do checksums come from for runtimes that do not publish them in a machine-
  readable form? Pin known-good hashes in the repo as a fallback?
- Do we upgrade in place, or install alongside and let the user switch? (Alongside is
  safer for PHP; probably in-place for Nginx and Composer.)
- GitHub API rate limits for the unauthenticated release lookups (RabbitMQ, Erlang).

## Out of scope

- The TUI screens that drive this (#1) — this issue is the engine
- Managing runtimes installed outside the stack root

## Dependencies

Depends on #3 (config model, stack-root resolution) and lands after #1 per the
roadmap's build order. Suggested implementation order within this issue: PHP first
(most versions, most benefit), then Nginx, Composer, MariaDB, and RabbitMQ/Erlang.

## Definition of done

- `devcrate install php 8.5` produces a working, configured PHP 8.5 that `start`
  serves on port 9085 — with no manual download step
- A fresh clone can reach a running stack without visiting any vendor website