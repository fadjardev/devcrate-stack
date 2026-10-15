# The dashboard

`devcrate` with no arguments opens an interactive terminal UI, built with
[ratatui](https://ratatui.rs/). It is the same executable as the subcommands in
[cli.md](cli.md), and every action it offers is one of those calls — nothing is
implemented twice.

```bat
devcrate
```

```
╭──────────────────────────────────────────────────────────────────────────────────────────────────────────╮
│ ◆ devcrate    1 Services   2 Sites   3 Logs     ● 3/4 up  ✗ 1 crashed  │  CLI PHP php-8.5  │  C:\devcrate │
╰──────────────────────────────────────────────────────────────────────────────────────────────────────────╯
╭ Services (4) ────────────────────────────────────────────────────────────────────────────────────────────╮
│  SERVICE     STATE         PORTS           UPTIME     PIDS        PATH                                   │
│                                                                                                          │
│▌ nginx       ● up          80 443          1h 06m     9184 +2     nginx\current\nginx.exe                │
│  PHP 7.4     ● up          9074            1h 06m     4212 +4     php\php-7.4\php-cgi.exe                │
│  PHP 8.2     ✗ crashed     (9082)          -          -           php\php-8.2\php-cgi.exe                │
│  MariaDB     ● up          3306            1h 06m     7768        mariadb\bin\mariadbd.exe               │
╰──────────────────────────────────────────────────────────────────────────────────────────────────────────╯
 ● ready
 s start   x stop   t restart   S X T whole stack   i install   u CLI PHP   ? help   q quit
```

Press `?` at any time for the key list.

## Why it exists

Everything on the Services table is available from `devcrate status` — except one
column value. A command runs once and exits, so it can only ever report what is
true *now*: a service that is not running reads as `stopped`, whether it was
never started or died thirty seconds ago. The dashboard stays resident, holds the
previous scan, and can therefore tell the difference. A service that goes from
`up` to `stopped` without anyone asking it to is shown as **`crashed`**.

That is the whole of the "real process supervision" constraint in
[roadmap](roadmap.md) item 1, and it is the one thing the subcommands genuinely
cannot do.

## Panes

| Key | Pane | What it does |
| --- | --- | --- |
| `1` | Services | Live state, ports, uptime, PIDs; start / stop / restart |
| `2` | Sites | The vhosts; create, change PHP version, delete |
| `3` | Logs | Tail any of the stack's log files |

`Tab` and `Shift-Tab` cycle. `↑` `↓` (or `j` `k`) move within a pane. `r` forces
a rescan rather than waiting for the next one.

### Services

| Key | Action |
| --- | --- |
| `s` `x` `t` | Start / stop / restart the **selected** service |
| `S` `X` `T` | ...the **whole stack**, in dependency order |
| `i` | Open the **Install Service / Runtime** picker — see [Installing and removing](#installing-and-removing) |
| `U` | Open the **Uninstall** picker: everything installed, removed after a confirmation |
| `u` | Repoint `php\current` — the CLI PHP version |

Start and stop go through the same order, the same graceful commands, and the
same path-scoped process matching as `devcrate start` and `devcrate stop`; see
[cli.md](cli.md) for what that means in detail. A port held by something that is
not ours is named under the table, with its PID.

Only one action runs at a time. A second `S` while the first is still starting is
ignored rather than queued — two `start`s racing is how you end up with a second
MariaDB pointed at a datadir the first one still holds.

Quitting while an action is in flight asks once. Answering `q` a second time
leaves anyway, which may abandon a shutdown halfway through.

### Installing and removing

`i` lists every runtime `devcrate install` can fetch: the newest five PHP
branches, nginx stable and mainline, Composer stable, MariaDB, PostgreSQL 13.23,
Redis, Erlang/OTP, RabbitMQ, Node.js LTS and current, Bun, Python 3.8, and
Adminer.
The labels name the exact release, read from the vendors' catalogues on a
thread of its own the first time the picker opens (the title shows a spinner
until they arrive). Offline, or when a catalogue cannot be read, that runtime
falls back to a keyword entry — "PHP (newest branch)", "nginx (stable line)" —
which resolves the same way `devcrate install php latest` does. While an
install runs, the footer shows its download and extraction progress.

The dashboard never forces. Choosing a version that is already installed is
refused with the `devcrate install ... --force` command to run instead, because
replacing a version folder replaces whatever was hand-edited inside it — a PHP
version's `php.ini`, for one.

`U` lists what is installed, by the same discovery `status` uses, and asks
before removing anything. It is `devcrate uninstall` without `--force` or
`--data`: a running service, the version a `current` junction names, a PHP
version a vhost still uses, Erlang under an installed RabbitMQ, and a
database's `data\` are all refused or kept, with the command that would do it
named in the result.

### Sites

| Key | Action |
| --- | --- |
| `n` | New vhost: a form for the hostname, project path (typed or picked with **Browse**), PHP version (or auto-detect), and whether to add the hosts entry and issue a certificate |
| `p` | Change the selected vhost's PHP version |
| `d` | Delete the vhost's conf (asks first) |

`n` is `devcrate site add`: the same web-root and PHP detection, the same hosts
block and UAC prompt, the same mkcert issuance — see
[cli.md](cli.md#devcrate-site-add--site-remove). `p` is `devcrate site set-php`:
it rewrites the `fastcgi_pass` line and nothing else, so a hand-edited conf
survives. `d` deletes only the conf — the project folder under `projects\` and
the certificate are left alone, and the confirmation says so.

`d` also takes the host out of the devcrate hosts block, as `devcrate site
remove` does — both call the same `site::delete`.

### Logs

The file list is discovered rather than fixed — every vhost adds an access and an
error log, and MariaDB and RabbitMQ name theirs after the machine. Error logs sort
first within each service, because when something is wrong that is the one you
want.

| Key | Action |
| --- | --- |
| `f` | Follow on / off |
| `PgUp` `PgDn` | Scroll |
| `Home` `End` | Top of the buffer / back to following |

Only the last 256 kB of a file is read, and at most 2000 lines are kept — an
nginx access log that has been growing for a month opens instantly. Scrolling up
turns following off, so the view stops moving under you while you read.

## How it stays responsive

Three background threads, so the drawing thread never blocks:

- The **scanner** re-reads the stack every two seconds. A scan is a full
  process-table walk plus a TCP connect per port, and a connect to a *dead* port
  costs the full 250 ms timeout — around two seconds with the stack down. That
  cannot happen inside a redraw.
- The **actor** runs one action at a time. These are slow by nature: a cold
  RabbitMQ node is allowed ninety seconds to answer.
- The **tailer** follows the selected log file and reports it only when it grows.

They send events; the UI thread folds them in and redraws. Actions nudge the
scanner when they finish, so the table catches up immediately rather than at the
next tick.

## The terminal

The alternate screen and raw mode are restored on every exit path, including a
panic: the panic hook restores the terminal *first* and then prints, so a crash
leaves you with a readable message and a working shell rather than a terminal
with no echo.

If stdout is not a terminal — piped, redirected, or in CI — the dashboard refuses
to start and points at `devcrate status --json` instead, rather than writing
escape sequences into a file.

## What it deliberately does not do

- **Force anything.** Replacing an installed version, removing the active one,
  and deleting a database's data are CLI-only (`--force`, `--data`), and the
  dashboard names the command when it refuses.
- **Install from a local archive.** `--from` is a CLI affordance.
- **Report certificate expiry.** The New Site form adds the hosts entry and
  issues the certificate, as `site add` does (see
  [cli.md](cli.md#what-this-deliberately-does-not-do)), but nothing shows when a
  certificate runs out.
- **Edit anything in a vhost except its PHP version.** The web root, the
  certificate, an added `location` block: still a manual edit. The dashboard, like
  the subcommands, never reformats or regenerates a conf you may have changed by
  hand.
- **Detect a crash it did not witness.** Supervision here is by observation
  between two scans, not by owning the process. A service that dies while the
  dashboard is closed reads as `stopped` the next time it opens, because that is
  all anything can honestly say about it.
