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

## On an empty stack

When the first scan finds nothing installed at all — a fresh clone or an
extracted release zip, where `php\php-X.Y\` holds only the stack's `php.ini` —
the dashboard opens on a **first-run screen** in place of the panes:

| Key | Action |
| --- | --- |
| `s` / `Enter` | Run [`devcrate setup`](cli.md#devcrate-setup): nginx, the newest PHP, MariaDB, Composer, and mkcert where missing, mkcert's CA trusted, the stack started |
| `Esc` | Go to the dashboard as it is, to install one thing at a time with `i` |
| `q` | Quit |

It is the same `setup::run` the command calls. Each step is ticked off on the
screen as it finishes, download progress shows beside the spinner, and a line
warns that the local-CA step waits on a Windows confirmation, which can open
behind the terminal window. When setup succeeds the screen gives way to the
dashboard, with the steps in the usual output box. When a step fails the screen
stays, showing the steps that finished and why it stopped; `s` carries on from
there, as running `devcrate setup` again would.

Only the first scan decides. A stack with anything installed — one service
row, even PostgreSQL alone — never shows the screen, and neither does one whose
services are all uninstalled later in the session.

## Panes

| Key | Pane | What it does |
| --- | --- | --- |
| `1` | Services | Live state, ports, uptime, PIDs; start / stop / restart |
| `2` | Sites | The vhosts; create, change PHP version, open, delete |
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
| `A` | Autostart on / off: `devcrate start` at logon (`devcrate autostart`) |
| `e` | On a PHP row: that version's **extensions**, switched on and off with `Enter` or `Space` |

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
Redis, Erlang/OTP, RabbitMQ, Node.js LTS and current, Bun, Python 3.8,
Adminer, and Mailpit.
The labels name the exact release, read from the vendors' catalogues on a
thread of its own the first time the picker opens (the title shows a spinner
until they arrive). Offline, or when a catalogue cannot be read, that runtime
falls back to a keyword entry — "PHP (newest branch)", "nginx (stable line)" —
which resolves the same way `devcrate install php latest` does. While an
install runs, the footer shows its download and extraction progress.

After an install or an uninstall the stack is read from disk again, so a PHP
version installed from here is listed as a row straight away, and the vhost and
log lists follow it — reopening the dashboard is not needed.

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

`e` is `devcrate php ext` for the PHP version under the cursor (on any other row
it says to pick a PHP one). The list is every DLL in the version's extension
directory and every extension its `php.ini` switches on, marked `on` or `off`;
`Enter` or `Space` flips the one selected, through the same `php ext enable` /
`disable` — the marked block, the `php -m` check, and the restart of that
version's worker if it was running. An extension the file itself switches on is
marked with its line, and flipping it is refused with that line named: only
devcrate's block is edited. When the version has no Xdebug DLL, the first row
is `get xdebug`, which is `php ext install xdebug` — the download matched to
that exact PHP build, with its progress in the footer. As with installs, the
dashboard never forces: replacing a DLL already there stays `php ext install
--force` in a terminal.

### Sites

| Key | Action |
| --- | --- |
| `n` | New vhost: a form for the hostname, project path (typed or picked with **Browse**), PHP version (or auto-detect), a proxy port (digits; set, the site forwards to `localhost:<port>` instead of PHP), and whether to add the hosts entry and issue a certificate |
| `N` | New Laravel or WordPress project: a form for the kind, the name, the PHP version, and the database |
| `p` | Change the selected vhost's PHP version |
| `v` | Start or stop the selected site's dev server (`devcrate dev`) |
| `R` | Issue the selected site's certificate again (`devcrate cert renew`) |
| `o` | Open the site in the default browser |
| `e` | Open the project folder in Explorer |
| `c` | Open a terminal in the project folder — Windows Terminal where installed, a cmd window otherwise |
| `d` | Delete the vhost's conf (asks first) |

A proxy vhost shows `→ :3000` in the PHP column. `p` on one turns it back into
PHP, writing its conf anew and keeping the old one as `<host>.conf.bak` — see
[cli.md](cli.md#proxy-vhosts).

`n` is `devcrate site add`: the same web-root and PHP detection, the same hosts
block and UAC prompt, the same mkcert issuance — see
[cli.md](cli.md#devcrate-site-add--site-remove). `p` is `devcrate site set-php`:
it rewrites the `fastcgi_pass` line and nothing else, so a hand-edited conf
survives. `d` deletes only the conf — the project folder under `projects\` and
the certificate are left alone, and the confirmation says so.

`N` is `devcrate new` — see [cli.md](cli.md#devcrate-new). The form previews
the host the name becomes and refuses, in place, a name that cannot be one;
WordPress offers MariaDB alone. The hosts entry and the certificate are always
on, as they are on the command line by default. Composer's and artisan's output
cannot go to the screen, so it goes to `logs\new-<name>.log` in the stack root,
which the Logs pane lists under `devcrate`; the footer names each step as it
starts, and the result names what was not finished, with the command that
finishes it.

The **CERT** column shows the site's certificate from each scan, judged as
`devcrate cert list` judges it: `✓ 2y` or `✓ 40d` while it is fine, `! 12d`
within 30 days of expiring, and `✗ expired`, `✗ name` (it does not cover the
host), `✗ CA` (another CA issued it), `✗ missing` or `✗ bad` otherwise; `-`
for a site without HTTPS. `R` runs `devcrate cert renew` for the selected site
— see [cli.md](cli.md#devcrate-cert). Starting the stack from the dashboard
renews the ones that need it by itself, as `devcrate start` does, unless
`[certs] auto_renew = false`.

The **DEV** column shows the site's dev server from each scan: `● :5173` while
it runs (the port it printed), `✗ exited` when it has gone without being
stopped, `-` when there is none. `v` runs `devcrate dev start` or `stop`,
whichever applies; its output is `logs\dev-<host>.log` in the Logs pane.

`d` also takes the host out of the devcrate hosts block, as `devcrate site
remove` does — both call the same `site::delete`.

**Auto-vhost.** While `[sites] auto` is on (the default), each scan also looks
for folders under `projects\` that no vhost serves, and serves them through
the same `devcrate site sync` — see [cli.md](cli.md#devcrate-site-sync). It
waits rather than interrupts: not while another action runs, a form or picker
is open, or the first-run screen is up. Each folder is tried once per session,
so one that fails is not retried — with its UAC prompt — every two seconds,
and a site deleted with `d` is not served straight back; it returns the next
time the dashboard opens unless its folder is in `[sites] ignore`.

`o` sends the browser to `https://<host>/`, or `http://` when the conf has no
`listen` on 443 or with `ssl`. `e` and `c` open the *project* folder, not the
web root: the conf's `root` is resolved against the nginx prefix and followed
through the `projects` junctions to the real directory, minus a trailing
`public` — so a site made with `--path D:\work\shop` opens `D:\work\shop`. The
terminal inherits the dashboard's environment, `PATH` included. None of the
three is queued behind a running action; they work mid-install, and the note
line says what was opened or why it could not be.

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
next tick. The stack the scanner and the actor share is swappable: which
services exist, and where nginx lives, are read from disk when the stack is
opened, so after an install, an uninstall, or setup the actor opens it again
and hands the new one to the scanner and the UI before that nudge.

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
- **Edit anything in a vhost except its PHP version and its certificate.** The
  web root, an added `location` block: still a manual edit. The dashboard, like
  the subcommands, edits a conf one line at a time; the one regeneration —
  turning a proxy vhost back into PHP with `p` — keeps the old conf as
  `<host>.conf.bak`.
- **Detect a crash it did not witness.** Supervision here is by observation
  between two scans, not by owning the process. A service that dies while the
  dashboard is closed reads as `stopped` the next time it opens, because that is
  all anything can honestly say about it.
