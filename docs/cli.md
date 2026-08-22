# The `devcrate` binary

The Rust program under [`devcrate/`](../devcrate) is the whole of the tool: an
interactive dashboard and a set of non-interactive subcommands, in one
executable, over one core.

- **`devcrate`** with no arguments opens the dashboard — see [tui.md](tui.md).
- **`devcrate <command>`** does one thing and exits. That is this document.

**Everything the batch scripts do is available in both**: `start`, `stop`,
`restart`, `php use`, `site add` / `set-php` / `remove`, and the reporting
commands. `install` goes further than any script does: `devcrate install php
8.4` downloads the release from windows.php.net, verifies its sha256 against
the vendor's own feed, and installs it, `devcrate install nginx 1.31.3` does the
equivalent from nginx.org, and `devcrate install composer` fetches the phar from
getcomposer.org and verifies it against the checksum the vendor publishes beside
it. `devcrate install postgres 13.23` downloads EDB's binaries zip and runs
`initdb` (a database service beside MariaDB), and `devcrate install python 3.8`
fetches the embeddable zip from python.org and bootstraps pip (a toolchain on
`PATH`, switched by `python use`). `--from` does the same from an archive already
on disk. MariaDB, RabbitMQ, Erlang, Node.js, and Bun are all installable end to
end too.

The batch scripts stay in the repo and keep working; nothing about them has
changed.

## Building it

```bat
cd C:\devcrate\devcrate
cargo build --release
```

The binary lands at `devcrate\target\release\devcrate.exe`. `target\` is
gitignored; the sources and `Cargo.lock` are tracked.

**Put it on `PATH`** by copying it to the stack root, which is already there for
the `.bat` helpers:

```bat
copy /Y devcrate\target\release\devcrate.exe C:\devcrate\devcrate.exe
```

Then `devcrate` works from any directory. Nothing else is needed: the stack root
is found from the executable's own location, so a `devcrate.exe` sitting *in*
the root resolves it immediately, with no `--root` and no `DEVCRATE_HOME`.

`/devcrate.exe` is gitignored — it is build output, not source. Repeat the copy
after each `cargo build --release`.

Requires a Rust toolchain (built against 1.96 and 1.97, edition 2024).
Dependencies: `clap`, `serde` + `toml`, `serde_json`, `sysinfo`, `anyhow`,
`ratatui` (with only the crossterm backend), `zip` (deflate only — the
bzip2/zstd/AES features pull in C libraries for formats no vendor here ships),
`ureq` with `rustls` for the downloads (blocking, like everything else here;
the baked-in webpki roots mean no dependence on the machine's certificate
store), `sha2` for verifying them, and `windows-sys` for the Win32 calls that
have no portable equivalent. No async runtime: the dashboard's three
background threads are threads, which is all the concurrency there is to
manage here.

## One core, two front ends

Each action exists once, as a function returning a structured result, with the
printing kept outside it — `control::run_start` beside `control::start`,
`site::create` beside `site::add`, and so on. The subcommand prints; the
dashboard renders. Neither reimplements the other, which is the constraint
[roadmap](roadmap.md) item 1 sets for the TUI.

## Finding the stack root

The batch scripts each resolve the stack root from their own location (`%~dp0`).
A single binary on `PATH` cannot rely only on that — it may well live outside the
stack it manages — so it tries, in order:

| # | Source | Notes |
| --- | --- | --- |
| 1 | `--root <path>` | Taken at its word; only has to exist. |
| 2 | `DEVCRATE_HOME` | Same. |
| 3 | The executable's own directory | Searches that folder and every folder above it. |
| 4 | The working directory | Same upward search. |

Steps 3 and 4 walk upward looking for a directory that *is* a stack root, so
`devcrate status` works from anywhere inside the tree — and a `cargo run` from
`C:\devcrate\devcrate` finds `C:\devcrate` on the way up.

A directory counts as a stack root if it holds `devcrate.toml`, or — which is
every stack the batch scripts have built so far — if it has a `php\` directory
alongside `start.bat` or an nginx prefix (a folder holding `conf\nginx.conf`).

`devcrate status` prints which of the four it used, so a wrongly guessed root is
visible rather than mysterious.

## Commands

| Command | Status |
| --- | --- |
| `devcrate` (no subcommand) | **works** — the dashboard, see [tui.md](tui.md) |
| `devcrate status [--json]` | **works** — installed / running / port holders |
| `devcrate config show` | **works** — prints the resolved config as TOML |
| `devcrate config path` | **works** — where `devcrate.toml` is read from |
| `devcrate php list` | **works** — installed versions, FastCGI ports, active CLI version |
| `devcrate site list` | **works** — vhosts with their `root` and FastCGI port |
| `devcrate start [service]` | **works** — preflights ports, starts in dependency order |
| `devcrate stop [service]` | **works** — graceful shutdown in the safe order |
| `devcrate restart [service]` | **works** — stop, then start |
| `devcrate php use <version>` | **works** — repoints `php\current` |
| `devcrate python list` | **works** — installed Python versions, and the active CLI one |
| `devcrate python use <version>` | **works** — repoints `python\current` |
| `devcrate nginx list` | **works** — nginx versions in the prefix, and the active one |
| `devcrate nginx use <version>` | **works** — repoints `nginx\current` |
| `devcrate nginx migrate` | **works** — moves a pre-restructure stack into the current layout |
| `devcrate site add <host>` | **works** — web root, conf, junction, reload |
| `devcrate site set-php <host> <version>` | **works** — repoints an existing vhost |
| `devcrate site remove <host>` | **works** — removes the conf, keeps the project |
| `devcrate install php <version>` | **works** — downloads, verifies sha256, installs |
| `devcrate install php` (no version) | **works** — lists the versions windows.php.net offers |
| `devcrate install nginx <version>` | **works** — downloads and installs a build into the prefix |
| `devcrate install nginx` (no version) | **works** — lists the builds nginx.org offers |
| `devcrate install composer <line\|version>` | **works** — downloads the phar, verifies sha256, installs it into `composer\` |
| `devcrate install composer` (no version) | **works** — lists the lines getcomposer.org offers |
| `devcrate install postgres <minor>` | **works** — downloads EDB's binaries zip (length/TLS), unpacks, runs `initdb` |
| `devcrate install python <branch\|release>` | **works** — downloads the embeddable zip (length/TLS), enables site, bootstraps pip |
| `devcrate install mariadb <version>` | **works** — downloads, unpacks, generates `my.ini` and `data\` |
| `devcrate install rabbitmq <version>` | **works** — downloads, unpacks, enables the management plugin |
| `devcrate install erlang <version>` | **works** — downloads, unpacks the RabbitMQ dependency |
| `devcrate install node <version>` / `devcrate install bun <version>` | **works** — downloads and installs into `node\` / `bun\` |
| `devcrate install <runtime> --from <file>` | **works** — the same install from an archive or phar on disk |
| `devcrate uninstall <runtime> [version]` | **works** — see [`devcrate uninstall`](#devcrate-uninstall) |

`--root` is accepted on every command.

Wherever a PHP version is named — `php use`, `site add --php`, `site set-php`,
and the service argument to `start` / `stop` / `restart` — it is matched on its
digits, so `8.5`, `85`, `php-8.5`, and the older `php85` all mean the same
version. Python versions match the same way (`3.8`, `38`, `python-3.8`). nginx
versions are matched differently, on the dotted prefix; see
[`devcrate nginx`](#devcrate-nginx) for why.

Colour is used only as a second channel for the state column and is turned off
automatically when the output is not a terminal, so `devcrate status > report.txt`
contains no escape sequences. `NO_COLOR` disables it outright and
`CLICOLOR_FORCE` keeps it on through a pipe. Nothing is wrapped or truncated to
a guessed width: when the terminal will not report one, long lines stay long.

### `devcrate status`

```
Devcrate  C:\devcrate
  root from  executable location
  config     built-in defaults (no devcrate.toml)
  CLI PHP    php-8.5  (via php\current)

  SERVICE   STATE      PORTS           UPTIME  PIDS     PATH
  nginx     up         80 443          3h 21m  9184 +2  nginx\current\nginx.exe
  PHP 7.4   up         9074            3h 21m  4212 +4  php\php-7.4\php-cgi.exe
  PHP 8.2   port busy  9082            -       -        php\php-8.2\php-cgi.exe
  PHP 8.5   stopped    (9085)          -       -        php\php-8.5\php-cgi.exe
  MariaDB   up         3306            3h 21m  7768     mariadb\bin\mariadbd.exe
  RabbitMQ  absent     (5672) (15672)  -       -        rabbitmq\sbin\rabbitmq-server.bat

  (port) = not answering

  port 9082 is held by Docker Desktop Backend.exe (pid 21440)

  7 vhosts: ic-stokdigital.test, hris.qhomedata.test, ...
```

**States** come from combining a TCP connect to `127.0.0.1:<port>` with a scan of
running processes:

| State | Meaning |
| --- | --- |
| `up` | Our process is running and at least one of its ports answers. |
| `starting` | Our process is running but nothing answers yet. |
| `port busy` | A port answers, but no process of ours is behind it — something else holds it, and starting the stack will fail on that port. |
| `stopped` | Installed, not running. |
| `absent` | Not installed in this stack root. |

"Our process" means the executable lives at, or under, the path listed in the
PATH column. Matching on the path rather than the image name is what keeps a
system-wide XAMPP `nginx.exe` from being reported as this stack's. It is also
what tells the PHP versions apart: each FastCGI worker runs from its own
`php\php-X.Y\php-cgi.exe`.

**UPTIME** is the age of the process in the PID column, in the two largest units
that apply (`45s`, `12m 30s`, `3h 21m`, `2d 05h`). It is the leader's age rather
than the pool's, because `PHP_FCGI_MAX_REQUESTS=500` recycles workers underneath
it — a worker's age would jitter for reasons that have nothing to do with the
service.

**Port holders.** When a port answers but nothing of ours is behind it, the
kernel's TCP table (`GetExtendedTcpTable`) is consulted for the owning PID and
its image name is printed under the table. That is the difference between
knowing `devcrate start` will fail and knowing what to close. No elevation is
needed; the listener table is readable by any process.

Two consequences worth knowing:

- `PHP_FCGI_CHILDREN=4` means each PHP version shows several PIDs. The column
  shows the **supervisor** and `+N` for the rest — the process whose parent is
  not itself one of the matches, so the `php-cgi.exe` that forked the pool
  rather than one of its workers, and the nginx master rather than a worker.
  Sorting by PID would pick whichever was numbered lowest, which on Windows is
  a worker as often as not.
- RabbitMQ runs as an Erlang node, so its process is `erl.exe` out of the
  stack's `erlang\` directory, not anything under `rabbitmq\`. Two more show up
  under it — `inet_gethost.exe` and `win32sysinfo.exe`, port programs the VM
  opens and closes with itself, so they are counted but never lead.
  `epmd.exe` is excluded from the match entirely: the port mapper outlives the
  broker (which is why `stop.bat` kills it separately), so counting it would
  report a stopped RabbitMQ as running.

The port probe opens and immediately drops a real TCP connection — the only check
that needs no privileges. A server that logs aborted connections (MariaDB does)
will note it.

`--json` emits the same information structured, for scripts.

### `devcrate start`

```
devcrate start           REM the whole stack
devcrate start php-8.5     REM one service
devcrate restart nginx   REM stop, then start
```

```
Starting the Devcrate stack in C:\devcrate

  MariaDB   listening in 1.5s
  PHP 7.4   listening in 0.5s
  PHP 8.2   listening in 0.5s
  PHP 8.5   listening in 0.5s
  RabbitMQ  listening in 7.5s
  nginx     listening in 0.3s

Started.
```

Startup order is the reverse of shutdown — MariaDB, the PHP pools, RabbitMQ,
nginx last — so nginx only starts once the backends it proxies to are answering.
`start.bat` approximates this with a flat `timeout /t 2` before nginx; this waits
for each port to actually answer, and reports how long it took.

Each service is launched the way `start.bat` launches it, including the details
that are easy to lose:

- PHP workers run with their own directory as the working directory, because
  `php.ini` for 7.4 and 8.2 uses a relative `extension_dir` and `error_log`.
  `PHP_FCGI_CHILDREN=4` and `PHP_FCGI_MAX_REQUESTS=500` are set the same way.
- The `nginx\projects` junction is recreated if missing, before nginx
  starts. See [troubleshooting.md](troubleshooting.md) for why it has to be a
  junction.
- RabbitMQ gets `ERLANG_HOME`, `RABBITMQ_BASE`, and `erlang\bin` on `PATH`, and
  is launched `-detached` with **no console** rather than a redirected one —
  OTP's terminal driver aborts with `nouser` if it is handed a console that is
  not a tty.

**Preflight.** Before launching anything, a service whose port is already
answering — when none of *our* processes are behind it — is reported and skipped
rather than started into a bind failure:

```
  PHP 7.4  FAILED: port 9074 is held by powershell.exe (pid 6316); not started
```

The holder is named from the kernel's TCP table. Where that lookup comes up
empty — a process that exited between the probe and the lookup — the message
falls back to "held by another process".

A service already running is reported as `already running` and left alone, so
`devcrate start` is safe to run twice.

If a process starts and then disappears, that is reported as
`exited immediately` — for PHP that is nearly always the missing Visual C++
Redistributable.

**One thing this fixes that `start.bat` cannot.** `devcrate start > log.txt`, or
piping it anywhere, returns as soon as the stack is up. The batch script appears
to hang forever in that situation: `start /B` hands the child every inheritable
handle, including the redirected stdout, so the pipe stays open for as long as
MariaDB or the Erlang node runs. `devcrate` clears the inherit flag on its own
standard handles before launching anything, so nothing downstream holds them.

Exit code 0 when every targeted service ended up listening, 1 otherwise.

### `devcrate stop`

```
devcrate stop            REM the whole stack
devcrate stop nginx      REM one service, by the id in `devcrate status`
devcrate stop php-8.5    REM ...which may also be spelled 8.5 or 85
devcrate stop php        REM every PHP version at once
```

```
Stopping the Devcrate stack in C:\devcrate

  nginx     stopped
  PHP 7.4   terminated (5 process(es))
  PHP 8.2   terminated (5 process(es))
  PHP 8.5   terminated (5 process(es))
  RabbitMQ  stopped, 1 helper process(es) cleared
  MariaDB   stopped

Stopped.
```

The order is `stop.bat`'s and for the same reasons: nginx first so no new
request reaches a backend that is about to vanish, then the PHP pools, then
RabbitMQ, then MariaDB last because it has the most to flush.

Each service is asked to shut itself down first — `nginx -s quit`,
`rabbitmqctl stop`, `mariadb-admin shutdown`, with the same `ERLANG_HOME` /
`RABBITMQ_BASE` environment `stop.bat` sets. Only what is still running
afterwards is terminated. A FastCGI pool has no shutdown command, so it is
terminated outright, exactly as the script does it.

Two differences from `stop.bat`:

- **It only stops this stack.** `taskkill /F /IM php-cgi.exe` kills every
  `php-cgi.exe` on the machine; this matches on the executable path, so a second
  stack or a system-wide MariaDB is left alone. Same for `erl.exe`, `epmd.exe`,
  and `mariadbd.exe`.
- **It waits only as long as it needs to.** The script pays a flat
  `timeout /t 4` after nginx whether or not nginx has gone. This polls, so a
  clean stack goes down in about a second; the per-service ceilings (10s for
  nginx, 30s for MariaDB and RabbitMQ) are only reached when something is stuck.

`epmd.exe` is cleared once the broker is confirmed down — it is excluded from
the running/stopped decision precisely because it outlives RabbitMQ, so
something has to clean it up. That is the "helper process(es) cleared" note.

Exit code 0 when every targeted service ended up stopped, 1 if any is still
running afterwards.

### `devcrate php use`

```
devcrate php use 8.5
devcrate php use 85       REM the same thing
devcrate php use php-8.5    REM also the same thing
```

```
CLI PHP -> PHP 8.5 (php\php-8.5)
  PHP 8.5.8 (cli) (built: Jul  1 2026 04:02:00) (ZTS Visual C++ 2022 x64)
```

What `phpuse.bat` does: repoint the `php\current` junction, which is what sits
on `PATH`, so `php`, `composer`, and `laravel` resolve to the chosen build. The
FastCGI workers are untouched — each vhost names its own port, so switching the
CLI version never changes what a site is served with.

It checks for `php.exe`, not `php-cgi.exe`: a version can serve FastCGI fine
while being useless on the command line. The existing junction is removed with a
call that deletes the reparse point rather than following it, so the PHP
installation it pointed at is never at risk; if `php\current` turns out to be a
real directory rather than a junction, the command refuses instead of deleting
anything.

The version is matched on its digits, so `8.5`, `85`, `php-8.5`, and the older
`php85` folder naming are all equivalent. `phpuse.bat` now does the same.

### `devcrate python`

```
devcrate python list             REM installed versions, and the active CLI one
devcrate python use 3.8          REM 3.8, 38, or python-3.8
```

```
CLI Python -> 3.8 (python\python-3.8)
  Python 3.8.10
```

Python is a **toolchain, not a service** — it has no long-lived listener, so it
is never started, stopped, or shown in the `status` service table (it appears
there as a one-line `Python 3.8 (via python\current)` instead). `python use` is
the exact analogue of `php use`: it repoints the `python\current` junction that
sits on `PATH`, so `python` and `pip` resolve to the chosen build. Put
`python\current` and its `Scripts\` on `PATH` once, the way `php\current` was
added.

Versions live in `python\python-<X.Y>` and are matched on their digits, so `3.8`,
`38`, and `python-3.8` are one version. Install one with
[`devcrate install python`](#devcrate-install).

### `devcrate nginx`

```
devcrate nginx list                REM the versions in the prefix, and the active one
devcrate nginx use 1.31.1          REM 1.31.1, nginx-1.31.1, or an unambiguous 1.31
devcrate nginx migrate [--dry-run] REM move a pre-restructure stack to this layout
```

```
  1.29.4  nginx\nginx-1.29.4
* 1.31.1  nginx\nginx-1.31.1

* = nginx\current -> the version that runs
```

**The prefix is `nginx\`, and it does not move.** It holds everything belonging
to the *stack* — `conf\` with the vhosts and certificates, `logs\`, `temp\`, and
the `projects` junction — while each nginx build gets its own folder inside it
and `current` names the active one.

This is the mirror image of PHP, deliberately. PHP's configuration is genuinely
per-version: each `php.ini` differs, three FastCGI workers run at once, and a
vhost picks one by port. So a PHP version is a self-contained folder. Only one
nginx runs, and its vhosts and certificates belong to the stack rather than to
whichever build is serving them — copying them per version would mean a version
switch quietly abandoned every site you had configured.

**No vhost conf mentions a version, and none is rewritten to switch.** That
falls out of nginx's two path bases both being outside the versioned folder:
`root`, `access_log`, and `error_log` resolve against the prefix, and
`ssl_certificate` against the conf directory. Both move as a unit, so
`root projects/myapp.test/public` and `certs/_wildcard.test.pem` mean the same
thing whichever build reads them.

`nginx use` resolves the same way `php use` does — remove the reparse point,
never follow it, and refuse if `current` turns out to be a real directory. The
version is matched on the *dotted* prefix rather than PHP's digits-only rule,
because nginx versions have three components and `digits("1.31")` and
`digits("1.3.1")` are the same string; `1.3` therefore selects the 1.3 series
and can never reach 1.31.something. Two releases of one series (`1.3.1` and
`1.3.2` with `1.3` asked for) is refused as ambiguous rather than guessed.

Switching while nginx is running says so: the change takes effect at the next
`devcrate restart nginx`, not immediately.

**`nginx migrate`** converts a stack built before this layout, where the
versioned folder *was* the prefix (`nginx-1.31.1\nginx.exe` beside
`nginx-1.31.1\conf\`). It lifts the parts belonging to the stack — the
certificates, `logs\`, `temp\`, and the whole `conf\` if the prefix has none
yet — up into `nginx\`, moves the build inside it, and creates the `current`
and `projects` junctions:

```
  drop the nginx-1.31.1\projects junction
  nginx-1.31.1\conf\certs -> nginx\conf\certs
  nginx-1.31.1\logs -> nginx\logs
  nginx-1.31.1\temp -> nginx\temp
  nginx-1.31.1 -> nginx\nginx-1.31.1
  link nginx\current -> nginx\nginx-1.31.1
  link nginx\projects -> projects
```

It refuses while nginx is running — moving the directory it is executing from
would leave it up with no configuration to reload — and does nothing on a stack
already in this layout, so running it twice is harmless. `--dry-run` prints the
plan and changes nothing. The build moves *last*, after the certificates and
logs have been lifted out of it, which is what makes them land in the prefix
rather than travel along inside the folder.

**One rough edge, stated rather than hidden:** between pulling the restructure
and running the migration, the tracked `conf\` is at `nginx\` while `nginx.exe`
is still in `nginx-1.31.1\`, so `devcrate status` reports nginx as `absent` —
it looks for the binary in the prefix, and it is not there yet. Nothing is
broken: a running nginx keeps serving, and `migrate` still finds it and refuses
until it is stopped. Migrating clears it.

**Both layouts keep working** either way. A directory is recognised as a prefix
by holding `conf\nginx.conf` — the file `-c conf/nginx.conf` resolves to —
rather than by its name, so an unmigrated stack still starts, and `start.bat`,
`stop.bat`, and `new-vhost.bat` resolve it the same way the binary does.

### `devcrate site add` / `site remove`

```
devcrate site add myapp.test --php 8.5
devcrate site add myapp.test              REM defaults to the current CLI version
devcrate site remove myapp.test
```

```
  created  projects\myapp.test\public
  created  projects\myapp.test\public\index.php
  wrote    nginx\conf\sites\myapp.test.conf
  reloaded nginx

https://myapp.test -> PHP 8.5 (fastcgi 9085)
```

The conf is the one `new-vhost.bat` writes, with the same prefix-relative
`root projects/<host>/public`, conf-relative `certs/`, and prefix-relative
`logs/`. The `projects` junction is recreated if missing, as the script does.

Differences from `new-vhost.bat`:

- **The PHP version comes from what is installed**, not from a port map
  hard-coded in the script. `--php` takes any of `8.5` / `85` / `php-8.5`, and
  omitting it uses whatever `php\current` points at.
- **It will not silently overwrite.** An existing conf for that host is an
  error until you pass `--force`.
- **The configuration is tested before the reload.** nginx loads every file in
  `sites\` as one document, so one bad conf fails the reload for *all* vhosts
  with nothing to say why. `nginx -t` runs first, and its message — file and
  line — is what you get instead.
- **A third-level domain is called out.** `api.mygroup.test` is not covered by
  the `*.test` wildcard, so the output tells you to issue `*.mygroup.test`
  rather than leaving you to find out from a browser warning.

The hosts-file entry is still yours to add, as with the script; automating it
needs elevation and is [roadmap](roadmap.md) item 4.

`site remove` deletes **only** the conf and reloads. The project folder under
`projects\` is never touched, and neither is the certificate.

Reloading right after a change is graceful: nginx keeps the old workers alive
until their connections finish, so a request made in the same instant can still
be served by the previous configuration. A second request gets the new one.

### `devcrate site set-php`

```
devcrate site set-php myapp.test 7.4
```

```
  nginx\conf\sites\myapp.test.conf : fastcgi 9085 -> 9074
  reloaded nginx

https://myapp.test -> PHP 7.4 (fastcgi 9074)
The FastCGI worker for PHP 7.4 has to be running: `devcrate start php-7.4`.
```

This is the only command that **edits** a conf rather than writing or deleting
one, so it edits as little as it can: the `fastcgi_pass` port, and the generated
`# PHP :` header comment so it does not go stale. Every other line comes through
byte for byte — including anything added by hand since the file was generated,
which is exactly what regenerating the file from a template would throw away.

Two things it deliberately will not do:

- A `fastcgi_pass` that points anywhere other than `127.0.0.1:` is left alone.
  Aiming a vhost at another host or a different backend is a deliberate choice,
  not a port to swap.
- A conf with no `fastcgi_pass` at all is reported as an error rather than
  written back unchanged, so a typo in the hostname does not look like success.

The configuration is tested with `nginx -t` before the reload, as with
`site add`. Changing the version does **not** start that version's FastCGI
worker — the output says which one you need.

### `devcrate install`

```
devcrate install php                              REM list what can be downloaded
devcrate install php 8.4                          REM download, verify, install
devcrate install php 8.4 --force                  REM replace an existing 8.4
devcrate install php --from C:\downloads\php-8.4.23-Win32-vs17-x64.zip
devcrate install php 8.4 --from <zip>             REM the file has been renamed

devcrate install nginx                            REM list what can be downloaded
devcrate install nginx 1.31.3                     REM download and install
devcrate install nginx 1.30                       REM ...or name the series
devcrate install nginx --from C:\downloads\nginx-1.31.3.zip

devcrate install composer                         REM list the lines on offer
devcrate install composer stable                  REM download, verify, install the current stable
devcrate install composer lts                     REM ...or the 2.2 LTS line
devcrate install composer --from C:\downloads\composer.phar

devcrate install postgres                         REM how to name a version (no index to list)
devcrate install postgres 13.23                   REM download EDB's binaries zip, unpack, initdb
devcrate install postgres --from C:\downloads\postgresql-13.23-1-windows-x64-binaries.zip

devcrate install python                           REM how to name a version (no index to list)
devcrate install python 3.8                       REM newest 3.8.x Windows build, then bootstrap pip
devcrate install python 3.8.10                    REM ...or an exact release
devcrate install python --from C:\downloads\python-3.8.10-embed-amd64.zip
```

The version is a positional argument, not a flag: `devcrate install php 8.4
--from <zip>` names the version *and* the archive. (`--version` is not accepted
here — it is clap's own flag on `devcrate` itself.)

```
  fetching the release list from windows.php.net
  downloading  100%  33.2 MB / 33.2 MB
  sha256 verified against the release list
  extracting   100%
  checking the build
  generating php.ini
  moving it into place

  77 files into php\php-8.4
  php.ini generated from php.ini-development, 11 extensions enabled

PHP 8.4.23 installed as php-8.4 (fastcgi 9084)
  serve a site with it   devcrate site add myapp.test --php 8.4
  make it the CLI PHP    devcrate php use 8.4
  start its worker       devcrate start php-8.4
```

**Five runtimes.** Naming one that is planned but not built (`mariadb`,
`rabbitmq`, `erlang`) says so and points at
[installation.md](installation.md); naming one that does not exist at all reads
differently, so a typo is not mistaken for a missing feature.

PHP and nginx share everything between the archive and the folder — the
extraction guard, the staging directory, the swap — and differ in what happens
at each end, because they are versioned for opposite reasons. Read
[`devcrate nginx`](#devcrate-nginx) for why the layouts mirror each other; the
consequence for installing is that PHP configures the *version* it just
unpacked, and nginx prepares the *prefix* the build is about to sit in.
Composer is the third shape and shares neither half: it is a single phar, not
an archive, and a tool the stack runs rather than a version it serves with, so
there is nothing to extract and nothing versioned — see [Composer](#composer)
below. **PostgreSQL** is a fourth: a database service whose single `postgres\`
directory holds a version-specific data cluster the install must never destroy.
**Python** is a fifth: a language toolchain, versioned like PHP but with no port
and no service — switched by a `python\current` junction, never started.

#### PHP

**The catalogue is the vendor's own `releases.json`** on windows.php.net, which
lists the current release of every branch — EOL branches included, so 7.4 is
still on it. With no version named, `devcrate install php` prints that list
(with what is already installed marked) and stops. Only each branch's current
release is offered: superseded patch releases move to the vendor's `archives/`
and install with `--from` instead. The thread-safe x64 zip is selected by the
`ts-` and `-x64` around the compiler tag, never by the tag itself, which
changes across branches (`vc15`, `vs16`, `vs17`).

**Every download is verified before it installs.** `releases.json` publishes a
sha256 for each zip; the transfer is hashed as it streams and a mismatch
discards it. The download lands in `_downloads\` (gitignored) under a `.part`
name and is only renamed to the real one after the hash matches — so a file
sitting in `_downloads\` under its final name is always a verified one. It is
also kept: the next install of the same release re-hashes it and skips the
transfer, which is what makes a once-downloaded release installable offline.

**The version names the folder.** It is read from the vendor's own file name —
`php-8.4.3-Win32-vs17-x64.zip` gives `php-8.4`, listening on 9084 by the usual
`90` + digits convention. Only the major and minor reach the folder name,
because that is what the stack knows a version by; the patch level goes in the
receipt. A renamed archive that carries no version is an error rather than a
guess, and naming the version on the command line overrides it in any of the
usual spellings.

**A non-thread-safe build is refused.** The stack runs `php-cgi.exe` as a
long-lived FastCGI listener with `PHP_FCGI_CHILDREN`, which needs the TS build
(see [php-versions.md](php-versions.md)). This is read from the unpacked files —
a TS build ships `php8ts.dll` — rather than from the file name, which is a poor
signal in both directions: the NTS download is the one marked `nts`, and the TS
one carries no marker at all.

**`php.ini` is generated from the release's own `php.ini-development`**, so
every comment and default the vendor shipped survives, and the result matches
the `php\php-8.5\php.ini` already in the repo: `extension_dir = "ext"`,
`error_log = php_errors.log`, and twelve extensions enabled — curl, exif,
fileinfo, gd, intl, mbstring, openssl, pdo_mysql, pdo_sqlite, sodium, sqlite3,
zip. The template comments the same key more than once with different values
(`extension_dir` as both `"./"` and `"ext"`), so lines are matched on the key
*and* the value; uncommenting on the key alone would enable whichever came last.
Any of the twelve the template has no line for is reported rather than silently
skipped.

**A missing Visual C++ runtime is a warning, not a failure.** `vcruntime140.dll`
in the system directory is checked for, because its absence is the single most
common cause of `php-cgi.exe` exiting with no output at all. It is a
good-enough signal rather than an inventory — it cannot report *which* version
is installed, only that nothing is.

#### nginx

```
  fetching the download page from nginx.org
  downloading  100%  2.7 MB / 2.7 MB
  sha256 934a8fedd91646a4d7c51079ac9b71397d1caf7a6aeb81ccfd361250c04ace2c
  (nginx publishes no checksum; the transfer was checked
   against its declared length over TLS to nginx.org)
  checking the build
  preparing the prefix
  moving it into place

  32 files into nginx\nginx-1.31.3
  created nginx\logs
  created nginx\temp
  created nginx\projects
  nginx\current -> nginx\nginx-1.31.3

nginx 1.31.3 installed as nginx-1.31.3
  start it   devcrate start nginx
  check it   devcrate nginx list
```

**The catalogue is the download page**, parsed, because nginx publishes no
machine-readable index of releases — no JSON, no plain-text listing. The parse
is kept to the two things the page has always done: put an `<h4>` above each
group, and link the Windows build as `/download/nginx-<version>.zip`. No tags
are matched and no nesting is tracked, so the source tarballs and PGP
signatures beside each zip are simply not that href. A page it finds *nothing*
in is an error rather than an empty list — a format change has to read as a
format change, not as "nginx has no releases".

`devcrate install nginx` with no version prints that list, labelled by the line
each release is on and with what is already installed marked:

```
  1.31.3  mainline
  1.30.4  stable
  1.28.3  legacy   installed
```

**A version may be named as a series.** `1.30` installs the current stable
release, by the same dotted-prefix rule [`devcrate nginx use`](#devcrate-nginx)
applies to the versions already in the prefix — and it is unambiguous here for a
reason particular to the page: it lists exactly one release per series. `1.3` is
therefore its own series and reaches neither 1.30 nor 1.31.

**The download is *not* checksum-verified, unlike PHP's**, and the output says
so rather than implying otherwise. nginx signs its releases with PGP and
publishes no hash, so there is nothing to compare the bytes against. What is
checked instead is the `Content-Length` the server declared — which catches the
failure that actually happens, a transfer cut short — over TLS to nginx.org. The
sha256 is computed locally, printed, and recorded in the receipt, so it can be
compared against the vendor's signature by hand if you want that. Verifying the
PGP signature properly would mean shipping and trusting nginx's signing keys,
which is a real feature rather than a detail, and it is not built.

**Installing a version does not silently become the version that runs.** A
prefix with no `current` yet activates the new build — there is nothing to
disturb, and an inactive lone build would do nothing at all. A prefix that
already names an active version is left alone, and the output says which
version still holds it and how to switch:

```
nginx 1.30.4 installed as nginx-1.30.4
  nginx\current still names nginx-1.31.3, so this build is installed
  but not the one that runs. To switch:
    devcrate nginx use 1.30.4
```

Installing the stable release beside the mainline one is an ordinary thing to
do, and it must not turn into a downgrade nobody asked for.

**The prefix is made startable.** `logs\` and `temp\` are created, and the
`projects` junction if it is missing. `logs\` is the load-bearing one: nginx
opens `logs/error.log` before it creates any of the paths in its configuration,
so a prefix without that directory fails at startup with a `CreateFile()` error
and no server.

**`conf\nginx.conf` is deliberately not generated.** It is tracked in the
repository, it belongs to the stack rather than to any build, and seeding it
from the vendor's default would produce a working nginx that includes no
`sites\` and serves none of your vhosts — a failure that looks like success. A
missing one is a warning naming the likely cause (an incomplete checkout)
instead.

**The new build is asked to parse your configuration.** `nginx -t` runs against
the prefix and its verdict is reported, but only when it fails, and it never
fails the install. On a fresh stack it *will* fail — `ssl_certificate` names a
file mkcert has not issued yet — which is why it is advisory. What it catches is
the case worth catching at install time rather than at the next `devcrate
start`: a newer nginx that no longer accepts a directive your existing vhosts
use.

**A pre-restructure stack is refused**, before the network call, with the
sequence that fixes it. Where the versioned folder *is* the prefix, installing
a second build would nest one version inside another and leave the stack's
certificates and logs under a build that is no longer the only one — exactly
the tangle [`nginx migrate`](#devcrate-nginx) exists to undo.

#### Composer

```
  fetching the version list from getcomposer.org
  fetching the checksum for 2.10.2
  downloading  100%  3.5 MB / 3.5 MB
  sha256 verified against getcomposer.org's checksum
  checking the phar
  writing the composer shim
  moving it into place

  1 file into composer
  wrote composer\composer.bat
  wrote composer\composer
  created composer\home
  created composer\cache

Composer 2.10.2 installed in composer
  Composer version 2.10.2 2025-06-...
  put it on PATH   add composer (once, like php\current)
  then             composer --version
```

**The catalogue is `getcomposer.org/versions`, JSON** — machine-readable, unlike
nginx's page, but carrying no hash, unlike PHP's feed. It lists the current
release of each maintained line, so `devcrate install composer` with no version
prints them and stops:

```
  stable    2.10.2   php >= 7.2.5
  LTS       2.2.29   php >= 5.3.0
  preview   2.10.2   php >= 7.2.5
  snapshot  bb38c69cac82c…
```

A line is named by keyword — `stable` (what you almost always want), `lts` for
the 2.2 series that still runs on old PHP, `preview`, `snapshot` — or an exact
version while it is still one of those the list carries. There is no series
shorthand like nginx's, because there is no series of point releases to
disambiguate; an older release installs with `--from`.

**The download *is* checksum-verified, at full strength.** The hash is not in
the catalogue, so it is fetched from the `composer.phar.sha256sum` sidecar the
vendor publishes beside each versioned phar, over the same TLS, and the transfer
is hashed as it streams — a mismatch discards it, exactly as PHP's does. This is
where Composer differs from nginx and the output says which you got: nginx has
only a length to check, Composer has a real hash. (The roadmap had expected
`installer.sig` for this; that is the SHA-384 of the *setup script*, which would
need the PHP bootstrap to use, whereas the sidecar hashes the phar the stack
actually installs.)

**It lands in `composer\`, and is not versioned.** Composer is one file that
runs under whatever PHP is current, not a runtime the stack serves with, so —
unlike PHP and nginx — there is no version folder, no `current` junction, and no
port. The phar goes to `composer\composer.phar`, beside the `home\` and `cache\`
that hold `COMPOSER_HOME` and `COMPOSER_CACHE_DIR`
([architecture.md](architecture.md)). Two shims are written next to it —
`composer.bat` for cmd/PowerShell and an extension-less `composer` for Git Bash
— each of which runs the phar with *bare* `php`, so Composer resolves through the
same `php\current` on `PATH` that `php` itself does. Add `composer\` to `PATH`
once, the way `php\current` was added.

**Installing over an existing Composer updates it**, rather than refusing the way
a version does. A phar already there is a tool to replace, not a version to
protect, so it is overwritten and the previous version reported (`updated from
2.7.1`). `--force` is therefore not needed for Composer, and the receipt beside
the phar records what is there so an update can say what it replaced without
running anything.

**The installed phar is run once, advisorily.** With a `php\current` to run it,
`composer --version` is invoked and its banner printed — the real proof the PHP
on `PATH` can run this release. A failure there (an old PHP, or none) is a
warning, never a reason to undo the install, the same way nginx's `-t` is. With
no PHP installed yet, the install still completes and says Composer needs one.

#### PostgreSQL

```
  locating PostgreSQL 13.23 on get.enterprisedb.com
  PostgreSQL 13.23 -> postgresql-13.23-1-windows-x64-binaries.zip
  downloading  100%  295.2 MB / 295.2 MB
  sha256 6e...  (EDB publishes no checksum; the transfer was checked
   against its declared length over TLS to the download host)
  extracting   100%
  checking the build
  initialising the cluster (initdb)
  moving it into place

  ... files into postgres
  cluster created in postgres\data (initdb)

PostgreSQL 13.23 installed as postgres
  superuser  postgres / postgres  (local dev default)
  change it  ALTER USER postgres PASSWORD '...';
  start it   devcrate start postgres        (listens on 5432)
  connect    psql -U postgres -h 127.0.0.1 -p 5432
  stop it    devcrate stop postgres
```

**There is no catalogue to list.** EDB serves the Windows x64 *binaries* zip
(the archive without the installer) from a predictable mirror URL, but publishes
no machine-readable index of versions and no checksum for the files. So
`devcrate install postgres` with no version does not print a list — it explains
how to name one, and notes that PostgreSQL 13 is end-of-life (final **13.23**,
2025-11-13). A named minor resolves to a URL directly; EDB's packaging build
number (`-1`, occasionally `-2`) is discovered by asking the mirror rather than
guessed.

**The download is length/TLS-checked, like nginx.** With no published hash,
there is nothing to verify the bytes against, so the transfer is checked against
its declared `Content-Length` over TLS and the sha256 computed locally and
recorded — never claimed as vendor-verified.

**One directory, and the cluster inside it must survive.** Unlike PHP, a
PostgreSQL data directory is written in a version-specific on-disk format, so
the builds cannot coexist — `postgres\` holds one version and its `data\`
cluster. The install protects that cluster: `--force` replaces the *binaries*,
and the swap lifts `data\` across to the new ones rather than letting it go with
the retired folder. A running server is refused rather than replaced underneath,
and a reinstall over an existing cluster (found by its `data\PG_VERSION`) leaves
it untouched.

**First-run `initdb` is part of installing.** A fresh install creates the
cluster with a localhost-development auth setup — `trust` for local connections,
`scram-sha-256` over TCP — and a default `postgres` / `postgres` superuser whose
password is printed rather than hidden. Change it with `ALTER USER` for anything
that leaves the machine. `initdb` needs no network, so `--from` installs it
offline too.

**It is a service, driven like MariaDB.** `devcrate start` / `stop` / `status`
and the dashboard treat it as one, on port 5432, in the database layer of the
start/stop order. It is launched by running `postgres.exe -D data` directly and
stopped with `pg_ctl stop -m fast`.

#### Python

```
  locating Python 3.8 on python.org
  Python 3.8.10 -> python-3.8.10-embed-amd64.zip
  downloading  100%  8.2 MB / 8.2 MB
  sha256 ab...  (python.org publishes no checksum; the transfer was checked
   against its declared length over TLS to python.org)
  checking the build
  enabling site (for pip)
  moving it into place

  32 files into python\python-3.8
  enabled `import site` (so pip can be installed and found)

Python 3.8.10 installed as python-3.8
  pip installed
  make it the CLI Python   devcrate python use 3.8
  then                     python --version
```

**A toolchain, not a service.** Python has no port and no long-lived listener,
so it is never started or stopped and never appears in the `status` service
table (it shows as a `Python 3.8 (via python\current)` line). It is versioned
like PHP — `python\python-<X.Y>` folders, matched on the digits — and activated
by [`devcrate python use`](#devcrate-python), which repoints `python\current` on
`PATH`. Installing is not activating.

**There is no catalogue to list**, and the download is length/TLS-checked.
python.org publishes MD5 and GPG for these files, not sha256, and serves no
version index. So `devcrate install python` with no version explains how to name
one; a branch (`3.8`) is resolved to its newest Windows build by asking
python.org's archive which patch levels still ship an embeddable zip (3.8 ends
at **3.8.10** — later 3.8.x are source-only), and an exact release (`3.8.10`) is
fetched directly.

**The embeddable distribution is made pip-capable.** It ships with `import site`
commented out in its `._pth`, which disables `site-packages` and pip; the
install uncomments that line, then fetches the *branch-specific*
`bootstrap.pypa.io/pip/3.8/get-pip.py` (the generic script targets the newest
Python and fails on 3.8) and runs it. Bootstrapping pip is the one networked
step, so it degrades to a message rather than a failure when there is none — the
Python is installed and usable, and pip can be finished later. Put
`python\current` **and its `Scripts\`** on `PATH` once, the way `php\current`
was added.

> Python 3.8 is end-of-life (final Windows build 3.8.10, May 2021). It is
> supported here for legacy projects locked to it; prefer a maintained branch
> (`devcrate install python 3.12`) for anything new.

#### Both PHP and nginx

**Nothing half-installed is ever visible.** The archive is unpacked into
`.devcrate-staging-<folder>` beside where it will land — `php\` for PHP, the
prefix for nginx — and only renamed into place once it has been checked and
configured. The leading dot is load-bearing for both: versions are discovered by
scanning `php\` for directories starting with `php` and the prefix for ones
starting with `nginx`, so a directory called `php-8.4` or `nginx-1.31.3` appears
in `status`, the version lists, and the dashboard the instant it exists. Any
failure clears the staging directory. Replacing a version with `--force` moves
the old one aside rather than deleting it, and puts it back if the rename fails,
so a failed install cannot leave the version missing altogether.

**Extraction cannot write outside the destination.** A zip entry carries its own
path, and that path came from a file downloaded off a vendor site, so it is
treated as input rather than instruction: an absolute path, a drive letter, a
colon, or a `..` fails the whole install rather than being sanitised. Beyond the
obvious reason, a `..` that survived into a document root would produce a PHP
that answers nothing but "No input file specified" — see
[troubleshooting.md](troubleshooting.md).

The extractor also strips the archive's own wrapper directory when every entry
is under one, which is what nginx's zip needs (`nginx-1.31.3/…`) and PHP's does
not (files at the top level). A single file *beside* the candidate wrapper means
there is no wrapper, because stripping would silently drop that file.

**Downloads are cached and reused.** They land in `_downloads\` (gitignored)
under a `.part` name and are renamed only once the transfer has passed whatever
check the vendor makes possible, so a file there under its final name is one
that completed. A repeat install of the same release reuses it and skips the
network — which is what makes a once-downloaded release installable offline. For
PHP the cached file is re-hashed against the feed and discarded if it no longer
matches; for nginx there is nothing to re-check it against.

Each install leaves a `.devcrate-install.toml` receipt in the version directory
recording the runtime, version, release, and the archive it came from —
including its sha256, computed locally, so a `--from` install gets one too, plus
thread-safety for PHP. It is informational: nothing reads it back, because the
stack discovers versions from the folder name. That is what keeps unpacking a
folder by hand a complete way to install a version.

**Not delivered here**, and worth knowing before relying on it:

- **A `--from` archive is hashed but not judged.** Its sha256 goes in the
  receipt; it is not compared against the vendor's feed, because the archive
  may legitimately be a release the feed no longer lists. Only downloads are
  checked, and for nginx not even they carry a published hash.
- **No PGP verification.** nginx's `.asc` signatures are not fetched or checked;
  see above for what is checked instead.
- **No offline version listing.** A cached archive installs with no network, but
  both catalogues are fetched every time a list is printed or a version
  resolved.

### `devcrate uninstall`

```
devcrate uninstall <runtime> [version] [--data] [--force]
```

The mirror of `install`, one runtime at a time. `runtime` is the same name
`install` accepts. `version` is required for the versioned, side-by-side
runtimes (`php`, `nginx`, `node`, `bun`, `python`) and lists what is installed
when omitted; it is ignored for the ones that have only one install
(`composer`, `mariadb`, `postgres`, `rabbitmq`, `erlang`).

A service that is still running is always refused — Windows will not cleanly
delete a directory a process is executing out of, and `devcrate stop
<service>` fixes it. Two things need `--force` on top of that:

- Removing the version `php\current`, `nginx\current`, `node\current`,
  `bun\current`, or `python\current` names, since the CLI would resolve to
  nothing afterwards. The junction is cleared, not left dangling.
- Removing a PHP version a vhost's `fastcgi_pass` still points at, or removing
  `erlang` while `rabbitmq\` is still installed (Erlang is its dependency, not
  a peer).

`mariadb`, `postgres`, and `rabbitmq` keep a `data\` directory beside their
binaries — the same directory an `install --force` reinstall already lifts
across rather than overwrites (see [`devcrate install`](#devcrate-install)
above). `uninstall` treats it the same way: everything else is removed, and
`data\` is left in place unless `--data` is also given. `--data` alone is
refused; it has to be paired with `--force`, since deleting a database's data
directory is the one irreversible thing this command does.

## `devcrate.toml`

Optional, and every key in it is optional. A stack built by the batch scripts has
no `devcrate.toml` at all, so anything missing is discovered from the layout on
disk: the `nginx-*` directory, and one PHP entry per `php\php-*` folder
(`php\current` is skipped — it is the CLI junction, not a version, and the
downloaded archives beside them are files rather than directories).

Discovered PHP entries follow the convention the batch scripts use: version `7.4`
from the folder name `php-7.4`, and FastCGI port `90` + the version digits, so
`php-7.4` listens on 9074.

`devcrate config show` prints the fully resolved configuration in this format, so
you can pin what is currently being discovered:

```bat
devcrate config show > C:\devcrate\devcrate.toml
```

```toml
[nginx]
dir = "nginx"
ports = [80, 443]

[mariadb]
dir = "mariadb"
port = 3306

[rabbitmq]
dir = "rabbitmq"
erlang_dir = "erlang"
port = 5672
management_port = 15672

[[php]]
version = "8.5"
dir = "php/php-8.5"
fastcgi_port = 9085
```

Paths are relative to the stack root. Unknown keys are rejected rather than
ignored, so a typo is an error instead of a setting that silently does nothing.

## Exit codes

| Code | Meaning |
| --- | --- |
| 0 | Success. |
| 1 | Error (no stack root found, unreadable or invalid config, …). |
| 2 | Usage error (clap's own code). |
| 3 | Historical: "declared but not implemented yet". Nothing exits 3 any more — every declared command is built — but a script that checks for it loses nothing. |

`status` reports what it finds and exits 0 whether or not services are running.

## What this deliberately does not do

- **Never writes `devcrate.toml`.** `config show` prints to stdout and leaves
  redirecting it to you. `install` does not write one either, though it is the
  one command that could claim a reason to: a version it has just installed is
  already fully described by its folder name, so recording it in a config file
  would create a second source of truth for something discovery already knows.
- **Never installs outside the stack root.** Every runtime lands under the root
  the command resolved, and an archive that asks to write anywhere else fails
  the install.
- **Never parses an nginx config.** `site list` does a shallow scan for the two
  directives it wrote (`root`, `fastcgi_pass`) and ignores the rest, so a
  hand-edited conf is reported as it stands. `site add` writes a whole file or
  refuses, `site remove` deletes only a file it would have written itself, and
  `site set-php` rewrites one line and copies the rest through unchanged. No
  command reformats a conf or regenerates one from a template. Editing anything
  *other* than the PHP version — the web root, the certificate, an added
  `location` block — is still a manual edit; a general vhost editor belongs to
  [roadmap](roadmap.md) item 1.
- **Never touches your project code.** `site remove` leaves `projects\<host>\`
  exactly where it is.
- **Never edits the hosts file or issues a certificate.** Both need elevation or
  a managed mkcert, which are roadmap items 4 and 2.
- **No supervision from a subcommand.** `start` launches and walks away;
  `status` and `stop` find their targets by scanning, not by remembering what
  was launched. So a service that dies five minutes later is reported by
  `devcrate status` as `stopped`, never as *crashed* — a command that runs once
  can only describe the present. Noticing the change needs something resident,
  which is what the [dashboard](tui.md) is: leave it open and the same service
  reads `crashed`.
