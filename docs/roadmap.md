# Roadmap

Where Devcrate is going. Today it is a set of batch scripts plus configuration
that you drive by hand; the goal is a single executable that manages the whole
stack for you.

**Status legend:** `planned` - agreed, not started. `in progress` - being built.
`done` - shipped, see [CHANGELOG.md](../CHANGELOG.md).

| # | Item | Status |
| --- | --- | --- |
| 1 | Rust TUI (`ratatui`), shipped as a single executable | done — see [tui.md](tui.md) |
| 2 | Built-in runtime downloader / installer with version selection | done — PHP, nginx, Composer, MariaDB, RabbitMQ, Erlang/OTP, Node.js, Bun, PostgreSQL, and Python all done end to end |
| 3 | Runs from any terminal, scriptable as well as interactive | done — see [cli.md](cli.md) |
| 4 | Open an existing project: vhost + hosts entry + mkcert TLS, in one step | done — automated hosts block editing with UAC elevation, mkcert TLS issuance, smart web root & composer PHP constraint detection |
| 5 | Node.js and Bun as managed runtimes | done — Node.js & Bun installer, version switcher, npm isolation, and smart .nvmrc / package.json detection |

---

## 1. Rust TUI built with ratatui

Replace the batch-file workflow with one interactive terminal UI, written in
Rust using [ratatui](https://ratatui.rs/) and
[crossterm](https://docs.rs/crossterm/), compiled to a single
`devcrate.exe` with no runtime dependency.

**Built.** `devcrate` with no arguments opens it; the reference is
[tui.md](tui.md). Everything below is in it except the two exclusions noted at
the end of this section. `tokio` and `tracing` were not needed - three threads
and structured results cover it.

**What it should do**

- **Dashboard** - live status of every service (Nginx, each PHP FastCGI
  listener, MariaDB, RabbitMQ): running / stopped, PID, bound port, uptime.
- **Start / stop / restart** - per service and for the whole stack, replacing
  `start.bat` and `stop.bat` while keeping the same graceful-shutdown order
  (nginx `-s quit` -> PHP -> RabbitMQ via `rabbitmqctl stop` -> MariaDB via
  `mariadb-admin shutdown`).
- **Log viewer** - tail `nginx\logs\*.log`, the MariaDB error log, and
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

**What it does not do**, and why:

- **Edit anything in a vhost other than its PHP version.** `site set-php`
  rewrites one line and copies the rest through; a general editor for the web
  root, the certificate, or an added `location` block is a different job, and
  the design constraint above ("a hand-edited config never gets clobbered") is
  easier to keep by not writing one yet.
- **Issue certificates when creating a site.** That needs mkcert as a managed
  tool, which is item 2, so the wildcard is reused exactly as `new-vhost.bat`
  does and a third-level domain is called out instead.
- **Notice a crash it did not witness.** Supervision is by comparing one scan
  with the last, not by owning the process: a service that dies while the
  dashboard is closed reads `stopped` when it next opens. Owning the children
  would mean the dashboard could never be closed without taking the stack with
  it, which is a worse trade for a development stack.

---

## 2. Download and install runtimes from inside the program

Right now every runtime is fetched manually from a website and extracted by
hand ([installation.md](installation.md)). Devcrate should do that itself.

**Managed runtimes:** PHP, Nginx, MariaDB, PostgreSQL, RabbitMQ, Erlang/OTP,
Composer, Python.

**Built for PHP, end to end.** `devcrate install php 8.4` fetches the
catalogue from windows.php.net, downloads the thread-safe x64 zip, verifies
its sha256 against the vendor's own feed, and installs it: unpacked, NTS
builds refused, a `php.ini` seeded from the release's own
`php.ini-development`, and the result swapped into place only once it is
complete. `devcrate install php` with no version lists what can be
downloaded; `--from <zip>` runs the same install from an archive already on
disk. The reference is [cli.md](cli.md#devcrate-install).

The local-archive path was built before the downloader deliberately. It is
required scope either way — the offline fallback in the constraints below — and
it holds everything that is hard to change later: the extraction guard, the
staging-and-swap that keeps a half-written version invisible, the TS check, and
the `php.ini` generator. All of it is testable with no network. Fetching then
reduced to one seam, as intended: produce a verified file on disk, and hand it
to the pipeline that already worked.

The download uses `ureq` with `rustls`, blocking rather than async, which
matches the rest of the binary — the dashboard's concurrency is three threads,
and there is no runtime to justify. The catalogue turned out to need only
`releases.json`, not the vendor's `archives/`: the feed lists the current
release of every branch, EOL ones included, so everything this stack runs is
on it. Superseded patch releases are deliberately not offered — they still
install with `--from`.

**Built for nginx too.** `devcrate install nginx 1.31.3` downloads the Windows
zip from nginx.org and installs it into the prefix as one more versioned build;
`devcrate install nginx` lists what is offered, labelled mainline / stable /
legacy, and a series such as `1.30` names the current release on it.

Two things about it are worth knowing rather than assuming, because they are
where nginx differs from PHP and neither difference is hidden in the output:

- **The catalogue is HTML, parsed.** nginx publishes no machine-readable index
  of releases at all, so the download page is read for its `<h4>` headings and
  its `/download/nginx-<version>.zip` links, and a page it finds nothing in is
  an error rather than an empty list.
- **The download is not checksum-verified.** nginx signs releases with PGP and
  publishes no hash, so the verify-before-extract constraint below is met only
  in the weaker form available: TLS to the vendor's own host, and the declared
  `Content-Length`. The sha256 is computed locally, printed, and recorded in the
  receipt. Doing this properly means shipping and trusting nginx's signing keys
  — a real feature, and not built. This is stated in the command's own output
  rather than left for someone to discover.

The rest follows from the layout already being right: a build lands in
`nginx\nginx-<version>\`, the prefix gets the `logs\` and `temp\` directories
and the `projects` junction it needs to start, and `current` is taken only if
no other version already holds it — installing the stable release beside the
mainline one must not become a downgrade nobody asked for. The new binary is
asked to parse the stack's configuration (`nginx -t`) and the verdict reported,
advisory rather than fatal.

**Built for Composer too**, and it is the one that fills in the verification
story the other two only bracket. `devcrate install composer` reads
`getcomposer.org/versions` — machine-readable, unlike nginx's page — downloads
the current stable phar, and verifies it against the `composer.phar.sha256sum`
the vendor publishes beside it: a real hash, at full strength, where nginx had
only a length. Composer is a tool, not a runtime the stack serves with, so it
breaks the versioned-folder mould deliberately — one phar in `composer\`, two
shims that run it under whatever `php\current` names, and no `current` junction
of its own. Installing over an existing one updates it rather than refusing.

One assumption this repo held going in turned out wrong, and is worth recording:
Composer's trust root is *not* `installer.sig`. That file is the SHA-384 of the
*setup script*, usable only through the PHP bootstrap; the `sha256sum` sidecar
hashes the phar the stack actually installs, which is both stronger and needs no
PHP to check. What the vendor publishes decided it, as ever.

**Built for PostgreSQL and Python too.** `devcrate install postgres 13.23`
downloads EDB's Windows binaries zip, unpacks it, and runs `initdb` to create
the cluster — a database *service* beside MariaDB (port 5432, one `postgres\`
directory, because a cluster's on-disk format is version-specific), whose
`data\` a `--force` reinstall carries across rather than destroys. `devcrate
install python 3.8` fetches the embeddable zip from python.org, enables `import
site`, and bootstraps pip — a *toolchain* like the planned Node/Bun (item 5), no
port and no service, switched by a `python\current` junction. Both vendors
publish no sha256 and no version index, so both downloads are length/TLS-checked
like nginx, and a version is named rather than a catalogue listed.

**Still to build:** uninstall / prune.

**What it should do**

- **Version catalogue** - list the versions available for each runtime,
  resolved from the upstream sources rather than a list baked into the binary:

  | Runtime | Source |
  | --- | --- |
  | PHP | **built** — `releases.json` on `windows.php.net/downloads/releases/`, which carries every branch and its sha256; `/archives/` turned out not to be needed |
  | Nginx | **built** — `nginx.org/en/download.html`, parsed, because there is no machine-readable index; no checksums published, only PGP |
  | PostgreSQL | **built** — EDB's Windows x64 binaries zip on `get.enterprisedb.com`, a predictable per-release URL; no index and no checksum published, so length/TLS-checked like nginx and an exact minor is named |
  | Python | **built** — python.org's release archive (`www.python.org/ftp/python/`); MD5/GPG only (no sha256) and no index, so length/TLS-checked and a branch resolved by probing which patches ship an embeddable zip |
  | MariaDB | MariaDB downloads REST API |
  | RabbitMQ | GitHub releases (`rabbitmq/rabbitmq-server`) |
  | Erlang/OTP | GitHub releases (`erlang/otp`) - Windows installer / portable |
  | Composer | **built** — `getcomposer.org/versions` (JSON, no hash); verified against the per-version `composer.phar.sha256sum` sidecar, *not* `installer.sig` (which signs the setup script, not the phar) |

- **Install** - pick a version, download with a progress bar, verify the
  checksum/signature the vendor publishes, extract into the standard layout
  (`php\php-8.5\`, `nginx\nginx-1.31.1\`, `mariadb\`, ...), and generate the
  first-run config (a `php.ini` seeded from `php.ini-development` with the
  extension set Devcrate expects; `my.ini`; the FastCGI port).
- **Multiple versions side by side** for PHP specifically - installing 8.5
  must not disturb 7.4 or 8.2, and it should register the new FastCGI port
  (`90` + version digits) and offer it in the vhost editor and `phpuse`.
  **Built for nginx too**, differently: one stable `nginx\` prefix holds the
  vhosts, certificates, and logs, with the builds versioned inside it and
  `nginx\current` naming the active one. PHP coexists because each version is
  self-contained; nginx switches because everything worth keeping is outside
  the versioned folder. The same model will *not* fit MariaDB or RabbitMQ,
  whose on-disk data formats are version-specific.
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

## 4. Open an existing project: vhost, hosts entry, and local TLS

Today, pointing Devcrate at a project you already have on disk is a three-part
manual job: `new-vhost.bat` scaffolds a *new* folder and writes the conf, but it
only **prints** the hosts line for you to paste in as Administrator, and the TLS
certificate has to be issued with mkcert by hand
([nginx-vhosts.md](nginx-vhosts.md)). Devcrate should do all three.

**What it should do**

- **Open a project** - point at any existing folder (`devcrate site add
  <path>`, or a directory picker in the TUI), pick the hostname and PHP
  version, and get a working `https://<host>` without moving or copying the
  code. The folder does not have to live under `projects\` - the vhost `root`
  can point anywhere, subject to the constraint below.
- **Detect the web root** - offer `public/` when it exists (Laravel,
  CodeIgniter 4, Symfony), otherwise the project root (CodeIgniter 2/3), and
  suggest a PHP version from `composer.json`'s `require.php` constraint when
  there is one.
- **Manage the hosts file** - add `127.0.0.1  <host>` to
  `C:\Windows\System32\drivers\etc\hosts` directly, and remove it again when the
  site is deleted. This needs elevation, so:
  - write the entries inside a marked block
    (`# --- devcrate begin/end ---`) so the tool only ever touches its own
    lines and never rewrites a hand-edited hosts file;
  - back the file up before the first write;
  - request elevation for just that operation (UAC prompt) instead of
    requiring the whole tool to run as Administrator;
  - fall back to printing the line, as now, when elevation is declined.
- **Issue local TLS with mkcert** - install [mkcert](https://github.com/FiloSottile/mkcert)
  as a managed tool (item 2), run `mkcert -install` once to add the local CA to
  the Windows trust store, then issue a certificate per domain group into
  `nginx\conf\certs\` and wire the `ssl_certificate` lines up
  automatically.
  - Reuse an existing wildcard when one covers the host (`*.test` covers
    `myapp.test`), and issue a new one when it does not - third-level domains
    such as `api.mygroup.test` need `*.mygroup.test`, which is exactly the
    manual step called out in `new-vhost.bat` today.
  - Report certificate expiry in the dashboard and offer to reissue.
- **Remove a site** - delete the conf, the hosts entry, and optionally the
  certificate, then reload nginx. The project folder itself is never deleted.

**Constraints**

- Private keys stay out of version control - `conf/certs/` is already
  gitignored, and the tool must not move keys anywhere else.
- The hosts file is shared, system-wide state: never rewrite lines outside the
  Devcrate block, and always leave the file valid if the write is interrupted
  (write to a temp file, then replace).
- A project outside the stack root cannot be reached through the
  `nginx\projects` junction, and PHP-CGI on Windows rejects any
  `SCRIPT_FILENAME` containing `..` (see
  [troubleshooting.md](troubleshooting.md)). So for out-of-tree projects the
  tool must emit an absolute, dot-free `root`, or create a per-project junction
  under `projects\` - decide once and document it.

---

## 5. Node.js and Bun as managed runtimes

PHP projects increasingly need a JavaScript toolchain for their front end (Vite,
Laravel Mix, Tailwind). Devcrate should carry that too, with the same
portable, multi-version approach it uses for PHP - nothing installed
system-wide, everything inside the stack root.

**What it should do**

- **Install Node.js** from the official Windows zip builds
  (`nodejs.org/dist/`), several versions side by side under `node\v22\`,
  `node\v20\`, and so on, including the bundled `npm`. LTS and current lines
  both listed.
- **Install Bun** from GitHub releases (`oven-sh/bun`,
  `bun-windows-x64.zip`) into `bun\<version>\`.
- **Switch the active version** the same way `phpuse` works today: a
  `node\current` junction on `PATH`, so `node`, `npm`, and `npx` resolve to the
  selected version, plus `bun\current`. Commands:

  ```
  devcrate install node 22.11.0
  devcrate node use 22
  devcrate install bun
  devcrate bun use 1.2
  ```

- **Per-project version** - read `.nvmrc` or `package.json`'s `engines.node`
  when opening a project (item 4) and offer to switch or install the version it
  asks for.
- **Run dev servers under supervision** - `npm run dev` / `bun run dev` started
  from the TUI, appearing on the dashboard next to the PHP services with its
  output in the log viewer, so a Vite dev server is stopped along with the rest
  of the stack instead of being left behind in some other terminal.
- **Proxy a dev server through nginx** (optional, later) - a vhost switch that
  forwards `/` to a local Vite/Bun port with WebSocket upgrade for HMR, so hot
  reload works over the same `https://<host>.test` as PHP.

**Constraints**

- Bun on Windows is x64-only and younger than the rest of the stack; treat it
  as optional and never a dependency of the core stack.
- `npm` writes a global prefix and cache outside the stack root by default -
  point both inside it (`node\npm-global`, `node\npm-cache`) so nothing leaks
  into the user profile.
- Node and Bun are additions, not replacements: the stack must keep working
  with neither installed.

---

## Sequencing

1. Rust project skeleton, `devcrate.toml` config model, path/stack-root
   resolution, and the CLI subcommand surface (item 3's plumbing).
   **Done** - see [cli.md](cli.md). The read-only commands landed with it:
   `status` (including `--json`), `config show` / `config path`, `php list`,
   and `site list`. Every command that would change the stack was declared up
   front but exited 3, naming the batch script that did the job at the time —
   by now all of them are built, and nothing exits 3 any more.
2. Port the existing scripts behind those subcommands: `start`, `stop`,
   `php use`, `site add`. **Done** - see [cli.md](cli.md). Every batch script in
   the stack root now has an equivalent subcommand: `start` / `stop` /
   `restart`, `php use`, and `site add` / `site remove`. They act only on the
   processes belonging to their own stack root, `start` preflights the ports it
   is about to bind, and `site add` tests the configuration before reloading.
   The scripts stay in the repo and keep working.

   Note what this does *not* deliver. `start` spawns and exits, so nothing
   watches the children afterwards - the crash detection item 1 asks for needs a
   resident process that owns them, which arrives with the TUI.
3. Close the gaps item 1's dashboard needs from the core, so the TUI is a front
   end rather than a second implementation. **Done** - see [cli.md](cli.md).
   `devcrate status` reports each service's uptime; a busy port is attributed to
   the process holding it, by name and PID, from the kernel's TCP table; and
   `devcrate site set-php` changes an existing vhost's PHP version by rewriting
   one line, leaving hand-edits in the same file untouched. The output is also
   terminal-aware now - colour only when something can render it, and no width
   assumed when the terminal will not report one.

   What is still missing for item 1 is the part that cannot be built without a
   resident process: crash detection, and the log viewer.
4. The ratatui dashboard on top of that core (item 1). **Done** - see
   [tui.md](tui.md). `devcrate` with no arguments opens it: a live service
   table, start/stop/restart per service and for the stack, a vhost manager, the
   PHP switcher, and a log tail. Scanning and actions run on background threads,
   so nothing blocks the redraw, and the terminal is restored on exit *and* on
   panic.

   Being resident is what earns its keep: it holds the previous scan, so a
   service that goes from `up` to `stopped` with nobody asking it to is reported
   as **crashed** rather than merely stopped. No subcommand can say that.

   Still not done from item 1's list: editing anything in a vhost other than its
   PHP version, and issuing certificates - both belong to items 2 and 4.
5. The runtime installer, starting with PHP - it has the most versions and the
   most benefit - then Nginx, Composer, MariaDB, and RabbitMQ/Erlang (item 2).
   **Done for PHP, end to end** - see [cli.md](cli.md#devcrate-install).
   `devcrate install php 8.4` reads the catalogue from windows.php.net's
   `releases.json`, downloads the TS x64 zip, verifies its sha256 against that
   same feed, and installs it atomically; `--from <zip>` is the offline path
   through the identical pipeline. The version it produces is discovered by
   `status`, `php list`, and the dashboard with no config written.

   **Nginx followed**, into the layout that had been built to receive it: one
   stable `nginx\` prefix, versioned build directories inside it, and a
   `nginx\current` junction naming the active one. `devcrate install nginx
   1.31.3` downloads from nginx.org, unpacks into `nginx\nginx-1.31.3\`, makes
   the prefix startable, and takes `current` only if no other version holds it.
   That the layout was settled first is why the installer itself is small: it
   had somewhere obvious to put a build.

   Two honest gaps, both in the command's own output: nginx's catalogue is an
   HTML page rather than a feed, and nginx publishes no checksums, so the
   download is checked against its declared length over TLS instead of a hash.

   **Composer followed**, and closed the second of those gaps. `devcrate install
   composer` reads the `getcomposer.org/versions` JSON and verifies the phar
   against the `composer.phar.sha256sum` the vendor publishes beside it — a real
   hash, where nginx had only a length. It is the first runtime that is a *tool*
   and not a version: one phar in `composer\`, shims that run it under
   `php\current`, no versioned folder, and an install that updates rather than
   refuses. The trust root turned out to be the sidecar, not the `installer.sig`
   this repo had expected — that signs the setup script, not the phar.

   **MariaDB, RabbitMQ, and Erlang followed**, completing item 2's runtime list
   end to end. Not delivered: uninstall / prune.
6. The full site workflow on top of `site add`: hosts-file management and
   mkcert issuance/renewal (item 4). The mkcert half depends on the installer
   from step 5, since mkcert becomes a managed tool.
7. Node.js and Bun (item 5) - the installer and the `current`-junction switcher
   are the same machinery as PHP, so this is mostly a new runtime definition;
   dev-server supervision and the HMR proxy come after.

The batch scripts stay in the repo and keep working until the equivalent
subcommand is shipped and documented.
