# The `devcrate` binary

The Rust program under [`devcrate/`](../devcrate) is the first step of
[roadmap](roadmap.md) sequencing item 1: the crate skeleton, the `devcrate.toml`
config model, stack-root resolution, and the CLI subcommand surface.

**It is read-only today.** Nothing in it starts, stops, or writes anything —
every command that would change the stack is declared so the shape is settled,
but exits with a message pointing at the batch script that does the job now. The
batch scripts remain the way you run the stack.

## Building it

```bat
cd C:\devcrate\devcrate
cargo build --release
```

The binary lands at `devcrate\target\release\devcrate.exe`. `target\` is
gitignored; the sources and `Cargo.lock` are tracked.

Requires a Rust toolchain (built against 1.97, edition 2024). Dependencies:
`clap`, `serde` + `toml`, `serde_json`, `sysinfo`, `anyhow`. The `ratatui` /
`crossterm` / `tokio` set from the roadmap is not pulled in yet — there is no TUI
and nothing async to drive.

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
alongside `start.bat` or an unpacked `nginx-*` directory.

`devcrate status` prints which of the four it used, so a wrongly guessed root is
visible rather than mysterious.

## Commands

| Command | Status |
| --- | --- |
| `devcrate status [--json]` | **works** — installed / running / port holders |
| `devcrate config show` | **works** — prints the resolved config as TOML |
| `devcrate config path` | **works** — where `devcrate.toml` is read from |
| `devcrate php list` | **works** — installed versions, FastCGI ports, active CLI version |
| `devcrate site list` | **works** — vhosts with their `root` and FastCGI port |
| `devcrate start [service]` | not built — use `start.bat` |
| `devcrate stop [service]` | not built — use `stop.bat` |
| `devcrate restart [service]` | not built |
| `devcrate php use <version>` | not built — use `phpuse` |
| `devcrate site add <host>` | not built — use `new-vhost.bat` |
| `devcrate site remove <host>` | not built |
| `devcrate install <runtime>` | not built — see [installation.md](installation.md) |

`--root` is accepted on every command.

### `devcrate status`

```
Devcrate  C:\devcrate
  root from  executable location
  config     built-in defaults (no devcrate.toml)
  CLI PHP    php85  (via php\current)

  SERVICE   STATE    PORTS           PIDS     PATH
  nginx     up       80 443          9184 +2  nginx-1.31.1\nginx.exe
  PHP 7.4   up       9074            4212 +4  php\php74\php-cgi.exe
  PHP 8.2   stopped  (9082)          -        php\php82\php-cgi.exe
  MariaDB   up       3306            7768     mariadb\bin\mariadbd.exe
  RabbitMQ  absent   (5672) (15672)  -        rabbitmq\sbin\rabbitmq-server.bat

  (port) = not answering

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
`php\phpNN\php-cgi.exe`.

Two consequences worth knowing:

- `PHP_FCGI_CHILDREN=4` means each PHP version shows several PIDs. The column
  shows the lowest PID and `+N` for the rest.
- RabbitMQ runs as an Erlang node, so its process is `erl.exe` out of the
  stack's `erlang\` directory, not anything under `rabbitmq\`. `epmd.exe` is
  excluded from the match: the port mapper outlives the broker (which is why
  `stop.bat` kills it separately), so counting it would report a stopped
  RabbitMQ as running.

The port probe opens and immediately drops a real TCP connection — the only check
that needs no privileges. A server that logs aborted connections (MariaDB does)
will note it.

`--json` emits the same information structured, for scripts.

## `devcrate.toml`

Optional, and every key in it is optional. A stack built by the batch scripts has
no `devcrate.toml` at all, so anything missing is discovered from the layout on
disk: the `nginx-*` directory, and one PHP entry per `php\php*` folder
(`php\current` is skipped — it is the CLI junction, not a version).

Discovered PHP entries follow the convention the batch scripts use: version `7.4`
from the folder name `php74`, and FastCGI port `90` + the version digits, so
`php74` listens on 9074.

`devcrate config show` prints the fully resolved configuration in this format, so
you can pin what is currently being discovered:

```bat
devcrate config show > C:\devcrate\devcrate.toml
```

```toml
[nginx]
dir = "nginx-1.31.1"
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
dir = "php/php85"
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
| 3 | Command is declared but not implemented yet. |

`status` reports what it finds and exits 0 whether or not services are running.

## What this deliberately does not do

- No writes of any kind, including to `devcrate.toml`. `config show` prints to
  stdout and leaves redirecting it to you.
- No nginx config parsing. `site list` does a shallow scan for the two directives
  `new-vhost.bat` writes (`root`, `fastcgi_pass`) and leaves everything else
  alone, so a hand-edited conf is reported, never rewritten.
- No process supervision. Status is a point-in-time snapshot; the roadmap's
  crash detection needs the tool to own the child processes, which comes with
  `devcrate start`.
