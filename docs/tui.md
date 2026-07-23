# The dashboard

`devcrate` with no arguments opens an interactive terminal UI, built with
[ratatui](https://ratatui.rs/). It is the same executable as the subcommands in
[cli.md](cli.md), and every action it offers is one of those calls — nothing is
implemented twice.

```bat
devcrate
```

```
╭devcrate──────────────────────────────────────╮╭──────────────────────────────────────────────╮
│  1 Services    2 Sites    3 Logs             ││                 C:\devcrate   CLI PHP php-8.5│
╰──────────────────────────────────────────────╯╰──────────────────────────────────────────────╯
╭Services──────────────────────────────────────────────────────────────────────────────────────╮
│SERVICE    STATE       PORTS          UPTIME    PIDS       PATH                               │
│nginx      up          80 443         1h 06m    9184 +2    nginx-1.31.1\nginx.exe             │
│PHP 7.4    up          9074           1h 06m    4212 +4    php\php-7.4\php-cgi.exe            │
│PHP 8.2    crashed     (9082)         -         -          php\php-8.2\php-cgi.exe            │
│MariaDB    up          3306           1h 06m    7768       mariadb\bin\mariadbd.exe           │
╰──────────────────────────────────────────────────────────────────────────────────────────────╯
 ready
 s start  x stop  t restart  (SHIFT: whole stack)  u CLI PHP  ? help  q quit
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

### Sites

| Key | Action |
| --- | --- |
| `n` | New vhost: type a hostname, then pick a PHP version |
| `p` | Change the selected vhost's PHP version |
| `d` | Delete the vhost's conf (asks first) |

`p` is `devcrate site set-php`: it rewrites the `fastcgi_pass` line and nothing
else, so a hand-edited conf survives. `d` deletes only the conf — the project
folder under `projects\` and the certificate are left alone, and the confirmation
says so.

Creating a vhost still leaves the hosts-file entry to you, exactly as
`devcrate site add` and `new-vhost.bat` do. It needs elevation; automating it is
[roadmap](roadmap.md) item 4.

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

- **Install runtimes.** `devcrate install php 8.4` works as a subcommand
  ([cli.md](cli.md#devcrate-install)), but the dashboard has no screen for it
  yet — no version picker, no download progress pane.
- **Edit the hosts file or issue certificates.** Both need elevation or a managed
  mkcert — roadmap items 4 and 2.
- **Edit anything in a vhost except its PHP version.** The web root, the
  certificate, an added `location` block: still a manual edit. The dashboard, like
  the subcommands, never reformats or regenerates a conf you may have changed by
  hand.
- **Detect a crash it did not witness.** Supervision here is by observation
  between two scans, not by owning the process. A service that dies while the
  dashboard is closed reads as `stopped` the next time it opens, because that is
  all anything can honestly say about it.
