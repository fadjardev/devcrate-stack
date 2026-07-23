# Rust TUI built with ratatui, shipped as a single executable #1

## Summary

Replace the batch-file workflow (`start.bat`, `stop.bat`, `new-vhost.bat`, `phpuse.bat`)
with one interactive terminal UI written in Rust using [ratatui](https://ratatui.rs/)
and [crossterm](https://docs.rs/crossterm/), compiled to a single `devcrate.exe`
with no runtime dependency.

Roadmap item 1 of 3 — see [docs/roadmap.md](docs/roadmap.md).

**Status: not started.** No box below is ticked, and `devcrate` with no arguments
still prints help — `ratatui` and `crossterm` are not dependencies yet. Some of
the *core* each box needs does already exist in the CLI from #3, and per this
issue's own technical note the TUI is meant to be a front end over it rather than
a second implementation:

| Scope item | Core available today |
| --- | --- |
| Dashboard: status, PID, port | `devcrate status` |
| Dashboard: uptime | `devcrate status` — the leader's age, in the table and in `--json` |
| Dashboard: refresh off the UI thread | **no** — the scan is synchronous, which is the TUI's problem to solve |
| Start / stop / restart, shutdown order | `devcrate start` / `stop` / `restart` |
| Report a crashed child | **no** — nothing is resident to notice |
| Log viewer | no |
| Vhost list | `devcrate site list` |
| Vhost create | `devcrate site add` (cert reuse only; mkcert issuance is #2/#4) |
| Vhost edit: change PHP version | `devcrate site set-php` — rewrites the `fastcgi_pass` line only |
| PHP switcher: show active | `devcrate php list` |
| PHP switcher: repoint junction | `devcrate php use` |
| Preflight: detect port conflicts | `devcrate start` refuses a port held by another process |
| Preflight: name the holder | `probe::listeners()` — `GetExtendedTcpTable`, used by both `status` and `start` |

What is left in that table is what genuinely cannot exist until something stays
resident: crash detection, the log viewer, and moving the scan off the drawing
thread. Everything else the TUI needs is now a function call away.

## Motivation

Today you have to know which script does what, read `nginx-1.31.1\logs\*.log` in a
separate editor, and find out a service died only when a request fails. There is no
single place that answers "is the stack up, and on which ports?". A TUI makes the
stack's actual state visible and controllable from one screen.

## Scope

### Dashboard
- [ ] Live status per service — Nginx, each PHP FastCGI listener, MariaDB, RabbitMQ
- [ ] Show running/stopped, PID, bound port, and uptime for each
- [ ] Refresh without blocking the UI thread

### Service control
- [ ] Start / stop / restart per service and for the whole stack
- [ ] Preserve the current graceful-shutdown order: nginx `-s quit` -> PHP ->
      RabbitMQ (`rabbitmqctl stop`) -> MariaDB (`mariadb-admin shutdown`)
- [ ] Report a crashed child process instead of showing it as "up"

### Log viewer
- [ ] Tail `nginx-1.31.1\logs\*.log`, the MariaDB error log, and the RabbitMQ node log
- [ ] Filter by site / log file
- [ ] Follow mode with scrollback

### Vhost manager
- [ ] List `conf/sites/*.conf` with each site's hostname, PHP version, and web root
- [ ] Create a site — scaffold the web root, write the vhost, issue the mkcert
      certificate, print (or patch) the hosts entry — i.e. what `new-vhost.bat` does
- [ ] Change an existing site's PHP version (rewrite `fastcgi_pass`) and reload nginx

### PHP switcher
- [ ] Repoint the `php\current` junction, as `phpuse.bat` does
- [ ] Show which version the CLI currently resolves to

### Preflight
- [ ] Detect port conflicts before starting (80, 443, 3306, 5672, 15672, each `90xx`)
- [ ] Name the process holding a busy port

## Design constraints

- **Controller, not a new source of truth.** The TUI reads and writes the same
  `nginx.conf`, `sites\*.conf`, and per-version `php.ini` files documented in
  [docs/architecture.md](docs/architecture.md). A hand-edited config must never be
  silently clobbered or reformatted.
- **Real process supervision.** Track child PIDs so a crashed service is reported,
  not assumed healthy.
- **No hard-coded `E:\dev`.** Stack root, installed runtime versions, and port
  assignments come from a `devcrate.toml` (see #3).
- **Restore the terminal on exit *and* on panic** — leave the alternate screen and
  raw mode in a sane state.

## Technical notes

Proposed crates: `ratatui`, `crossterm`, `tokio`, `serde` + `toml`, `sysinfo`
(process and port inspection), `tracing`.

The TUI should be a front end over the same core the subcommands in #3 call — no
logic that exists only in the UI layer.

## Out of scope

- Downloading or installing runtimes (that is #2)
- The non-interactive subcommand surface (that is #3)
- Non-Windows support

## Dependencies

Depends on the config model and stack-root resolution from #3. Per the roadmap's
build order, #3's plumbing lands first, then this.

## Definition of done

- `devcrate` with no arguments launches the TUI
- Every workflow currently covered by `start.bat`, `stop.bat`, `new-vhost.bat`, and
  `phpuse.bat` is reachable from it
- The batch scripts still work and are documented as the fallback until each
  equivalent ships