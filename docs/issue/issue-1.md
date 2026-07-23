# Rust TUI built with ratatui, shipped as a single executable #1

## Summary

Replace the batch-file workflow (`start.bat`, `stop.bat`, `new-vhost.bat`, `phpuse.bat`)
with one interactive terminal UI written in Rust using [ratatui](https://ratatui.rs/)
and [crossterm](https://docs.rs/crossterm/), compiled to a single `devcrate.exe`
with no runtime dependency.

Roadmap item 1 of 3 — see [docs/roadmap.md](docs/roadmap.md).

**Status: built.** `devcrate` with no arguments opens the dashboard; the
reference is [docs/tui.md](docs/tui.md). It is a front end over the core from #3
as this issue's technical note requires — every action it offers is a call into
`control`, `php`, or `site`, and each of those was split into a function that
returns a structured result with the printing left outside, so nothing is
implemented twice and nothing the dashboard calls can write over the screen.

Two scope items below are deliberately left undone; both are marked and
explained where they appear.

| Scope item | Where it lives |
| --- | --- |
| Dashboard: status, PID, port | Services pane, from `status::report` |
| Dashboard: uptime | Services pane — the leader's age, so a recycled worker does not reset it |
| Dashboard: refresh off the UI thread | the scanner thread, every 2s, nudged after each action |
| Start / stop / restart, shutdown order | `s`/`x`/`t` and `S`/`X`/`T` → `control::run_start` / `run_stop` |
| Report a crashed child | Services pane — `up` → `stopped` with nobody asking |
| Log viewer | Logs pane — discovered files, follow, scrollback |
| Vhost list | Sites pane |
| Vhost create | `n` → `site::create` (cert reuse only; mkcert issuance is #2/#4) |
| Vhost edit: change PHP version | `p` → `site::repoint_site` |
| PHP switcher: show active | header, and the picker marks it |
| PHP switcher: repoint junction | `u` → `php::use_version` |
| Preflight: detect port conflicts | `control::start_service` refuses a port held by another process |
| Preflight: name the holder | `probe::listeners()` — `GetExtendedTcpTable`, shown under the table |

## Motivation

Today you have to know which script does what, read `nginx\logs\*.log` in a
separate editor, and find out a service died only when a request fails. There is no
single place that answers "is the stack up, and on which ports?". A TUI makes the
stack's actual state visible and controllable from one screen.

## Scope

### Dashboard
- [x] Live status per service — Nginx, each PHP FastCGI listener, MariaDB, RabbitMQ
- [x] Show running/stopped, PID, bound port, and uptime for each
- [x] Refresh without blocking the UI thread — a scanner thread on a 2s timer,
      nudged by each action so the table catches up immediately. It has to be
      off-thread: probing a dead port costs the full 250 ms connect timeout.

### Service control
- [x] Start / stop / restart per service and for the whole stack
- [x] Preserve the current graceful-shutdown order: nginx `-s quit` -> PHP ->
      RabbitMQ (`rabbitmqctl stop`) -> MariaDB (`mariadb-admin shutdown`) —
      unchanged, because it calls the same `control` functions the subcommands do
- [x] Report a crashed child process instead of showing it as "up". By comparing
      each scan with the last: `up` -> `stopped` that nobody asked for is
      `crashed`. A service *you* stopped is not, and nor is one going down
      inside an action that is still running.

### Log viewer
- [x] Tail `nginx\logs\*.log`, the MariaDB error log, and the RabbitMQ node log
- [x] Filter by site / log file — the file list is discovered, not fixed, so a
      vhost added this session appears once it has been requested
- [x] Follow mode with scrollback. Scrolling up turns follow off, so the view
      does not move under you while you read.

### Vhost manager
- [x] List `conf/sites/*.conf` with each site's hostname, PHP version, and web root
- [x] Create a site — scaffold the web root, write the vhost, print the hosts
      entry. **Not** the mkcert certificate: that needs mkcert as a managed tool,
      which is #2. The `*.test` wildcard is reused and a third-level domain is
      called out, exactly as `new-vhost.bat` does.
- [x] Change an existing site's PHP version (rewrite `fastcgi_pass`) and reload nginx

### PHP switcher
- [x] Repoint the `php\current` junction, as `phpuse.bat` does
- [x] Show which version the CLI currently resolves to

### Preflight
- [x] Detect port conflicts before starting (80, 443, 3306, 5672, 15672, each `90xx`)
- [x] Name the process holding a busy port

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

- [x] `devcrate` with no arguments launches the TUI
- [x] Every workflow currently covered by `start.bat`, `stop.bat`,
      `new-vhost.bat`, and `phpuse.bat` is reachable from it
- [x] The batch scripts still work and are documented as the fallback until each
      equivalent ships — none has been retired

## Design constraints, as met

- **Controller, not a new source of truth.** No conf is regenerated or
  reformatted. `site set-php` rewrites one `fastcgi_pass` line and copies the
  rest of the file through byte for byte; a hand-added `location` block survives
  a version change. Deleting a site removes only the conf.
- **Real process supervision.** By observation between scans rather than by
  owning the children — see the note in [roadmap.md](docs/roadmap.md) item 1 on
  why owning them would be the worse trade for a development stack. A service
  that dies while the dashboard is closed still reads `stopped` when it reopens,
  which is all anything can honestly claim about it.
- **No hard-coded `E:\dev`.** Unchanged; the dashboard takes the same resolved
  `Stack` as every subcommand.
- **Restore the terminal on exit *and* on panic.** Done, with the panic hook
  restoring before the original hook prints.