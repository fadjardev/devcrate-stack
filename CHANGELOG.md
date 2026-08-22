# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **`devcrate uninstall <runtime> [version]`** — The mirror of `install`, closing the one gap left in Roadmap Item 2. A versioned runtime (php, nginx, node, bun, python) is removed by deleting its folder, guarded by `--force` when it is the version `current` points at, or — for PHP — when a vhost's `fastcgi_pass` still names it. A single-directory service that keeps `data\` beside its binaries (mariadb, postgres, rabbitmq) has that data preserved by default, the same as an `install --force` reinstall; deleting it needs `--data` paired with `--force`. Erlang refuses to remove itself while RabbitMQ still depends on it, without `--force`. A service that is still running is always refused.
- **Interactive TUI Install Modal (Key `i`)** — Added one-click service & runtime downloader modal directly inside the TUI dashboard for Node.js, Bun, PHP, MariaDB, RabbitMQ, Composer, and Nginx.
- **Node.js & Bun Managed Runtimes (`devcrate install node`, `devcrate install bun`, `devcrate node`, `devcrate bun`)** — Implemented Roadmap Item 5: portable JavaScript runtimes and version management.
  - Downloads Node.js zip distributions from `nodejs.org/dist/` into `node/v<version>/` and Bun from GitHub Releases into `bun/v<version>/`.
  - Configures `npm-global` and `npm-cache` inside `node/` so global NPM packages and cache stay inside the stack root without cluttering `%AppData%`.
  - Version switching via `devcrate node use <version>` and `devcrate bun use <version>` using `node/current` and `bun/current` junctions.
  - Smart Node version detection from `.nvmrc` and `package.json` (`engines.node`).
- **Automated Hosts & TLS Management (`devcrate site add`)** — Implemented Roadmap Item 4: one-step project onboarding with automated Windows `hosts` file modification and `mkcert` TLS certificate issuance.
  - Modifies `C:\Windows\System32\drivers\etc\hosts` inside isolated `# --- devcrate begin/end ---` marker blocks, with automatic UAC elevation fallback via PowerShell when permissions are denied.
  - Automatically issues wildcard TLS certificates (`_wildcard.<domain>.pem`) into `nginx/conf/certs/` using `mkcert`.
  - Smart web root detection (`public/` directory for Laravel, CodeIgniter 4, Symfony).
  - Smart PHP version detection from `composer.json` (`require.php` constraint parsing).
  - Out-of-tree project support via `--path <DIR>` with automatic junction creation under `projects/`.
- **`devcrate install mariadb`, `rabbitmq`, and `erlang`** — Built-in automated installers and archive unpackers for MariaDB, RabbitMQ Server, and Erlang/OTP, completing Roadmap Item 2 end-to-end.
  - MariaDB creates a default `my.ini` and `data/` directory.
  - RabbitMQ sets up `data/` directory and enables `rabbitmq_management` plugin.
  - Erlang/OTP installs silently into `erlang/` for RabbitMQ dependency.

### Changed

- **The nginx directory is now a stable prefix with the versions inside it.**
  `nginx\` holds everything belonging to the stack - `conf\` (the vhosts and
  certificates), `logs\`, `temp\`, and the `projects` junction - while each
  build lives in `nginx\nginx-<version>\` and `nginx\current` names the active
  one. Previously the versioned folder *was* the prefix
  (`nginx-1.31.1\nginx.exe` beside `nginx-1.31.1\conf\`), so unpacking a second
  version would have created a second folder, silently changed which one
  discovery picked, and abandoned every vhost and certificate configured in the
  first.
  - **The mirror image of PHP, deliberately.** PHP's configuration is genuinely
    per-version - each `php.ini` differs, three FastCGI workers run at once, and
    a vhost picks one by port - so a PHP version is a self-contained folder.
    Only one nginx runs, and its vhosts and certificates belong to the stack
    rather than to the build serving them, so those stay in the prefix and the
    builds move underneath them.
  - **No vhost conf changed, and none has to change to switch versions.** That
    falls out of nginx's two path bases both being outside the versioned
    folder: `root` and the logs resolve against the prefix, `ssl_certificate`
    against the conf directory. `root projects/myapp.test/public` and
    `certs/_wildcard.test.pem` mean the same thing whichever build reads them.
  - **`devcrate nginx migrate` converts an existing stack**, with `--dry-run` to
    see the plan first. It lifts the certificates, `logs\`, and `temp\` up into
    the prefix - and the whole `conf\` if the prefix has none yet, which is the
    case when migrating before pulling - then moves the build inside and creates
    the `current` and `projects` junctions. It refuses while nginx is running,
    and does nothing on a stack already converted, so running it twice is
    harmless. The build moves *last*, after the certificates and logs are out of
    it, which is what makes them land in the prefix rather than travel along.
  - **Both layouts keep working.** A directory is recognised as a prefix by
    holding `conf\nginx.conf` - the file `-c conf/nginx.conf` resolves to -
    rather than by its name, so an unmigrated stack still starts. `start.bat`,
    `stop.bat`, and `new-vhost.bat` resolve it the same way the binary does, and
    none of them names a version any more.
  - The tracked configuration moved from `nginx-1.31.1\conf\` to `nginx\conf\`,
    and `.gitignore` follows: `nginx\conf\` is versioned, everything else under
    `nginx\` is not, and the old `nginx-*\` is ignored wholesale.
- **`devcrate nginx list` / `use` / `migrate`** - the versions in the prefix and
  which one is active, switching by repointing `nginx\current`, and the
  migration above. `use` matches on the *dotted* prefix rather than PHP's
  digits-only rule, because nginx versions have three components and
  `digits("1.31")` and `digits("1.3.1")` are the same string: `1.3` selects the
  1.3 series and can never reach 1.31.something, and two releases of one series
  are refused as ambiguous rather than guessed. Switching while nginx is running
  says the change waits for a restart.

- **PHP folders are named `php-7.4` / `php-8.2` / `php-8.5`** instead of
  `php74` / `php82` / `php85`, and any version installed later follows the same
  `php-<X.Y>` pattern. The dotted form matches how the version is written
  everywhere else, and is unambiguous for a two-digit minor (`php-8.10` versus
  `php810`).
  - Nothing has to be typed the new way. Every command that takes a version
    matches on its digits, so `8.5`, `85`, `php-8.5`, and the old `php85` are
    interchangeable - in `phpuse.bat`, `new-vhost.bat`, `devcrate php use`,
    `devcrate site add --php`, and the service argument to `devcrate start` /
    `stop` / `restart`.
  - `phpuse.bat` and `new-vhost.bat` now resolve the version against the
    `php\php-*` folders that actually exist rather than a list baked into the
    script, so a newly unpacked version works without editing either of them.
    `new-vhost.bat` derives the FastCGI port the same way `start.bat` assigns
    it (`90` + the digits) in place of its hard-coded three-entry port map.
  - Existing vhost confs are unaffected: they name a port, not a folder.
- **The stack is now location-independent** - it runs from any folder (the
  *stack root*), not just `E:\dev`:
  - `start.bat`, `stop.bat`, `phpuse.bat`, and `new-vhost.bat` resolve the
    stack root from their own location (`%~dp0`) instead of hard-coding it.
  - Vhost confs use nginx-prefix-relative paths: `root projects/<domain>`
    (through a `nginx\projects -> ..\projects` junction that
    `start.bat`/`new-vhost.bat` auto-create), `certs/...` (conf-relative), and
    `logs/...`. The junction exists because PHP-CGI on Windows rejects `..` in
    `SCRIPT_FILENAME` ("No input file specified"), so roots must stay dot-free.
  - `php.ini` for 7.4/8.2 now uses relative `extension_dir = "ext"` and
    `error_log = "php_errors.log"`; `start.bat` launches each `php-cgi.exe`
    with its own folder as working directory so relative paths resolve there.
  - Documentation now uses `C:\devcrate` as a stand-in example root and
    describes the stack-root convention; first-time CLI setup is `phpuse 8.5`
    (which creates the `php\current` junction) instead of a manual `mklink`.
  - The only machine-specific absolute paths left are outside the repo: user
    `PATH` entries and the `php\current` junction target.

### Added

- **PostgreSQL 13 is a managed service.** `devcrate install postgres 13.23`
  downloads EDB's Windows x64 *binaries* zip, unpacks it into `postgres\`, and
  runs `initdb` to create the cluster; `devcrate start` / `stop` / `status` and
  the dashboard then drive it beside MariaDB, on port 5432.
  - **A near-twin of MariaDB, and placed the same way.** A new `ServiceKind`,
    started by running `postgres.exe -D data` directly (not `pg_ctl`, which forks
    the server and exits, so the process left to supervise is not the one
    serving), stopped gracefully with `pg_ctl stop -m fast`. It joins the
    database layer of the start/stop order - up before PHP and nginx, down after
    them.
  - **One directory, not versions side by side.** A PostgreSQL data directory is
    written in a version-specific on-disk format, so the builds cannot coexist
    the way PHP's do; `postgres\` holds one version and its `data\` cluster. The
    install is careful never to walk over an existing cluster: `--force` replaces
    the *binaries*, and the swap lifts `data\` across to the new ones rather than
    letting it go with the retired folder. A running server is refused rather
    than replaced underneath.
  - **First-run `initdb` is part of installing.** A fresh install creates the
    cluster with a localhost-development auth setup - `trust` for local
    connections, `scram-sha-256` over TCP with a default `postgres` / `postgres`
    superuser whose password is printed, not hidden. A reinstall finds the
    existing cluster (by its `PG_VERSION` marker) and leaves it untouched.
  - **The download is length/TLS-verified, and says so.** EDB serves these zips
    from a predictable mirror URL but publishes no checksum and no machine-
    readable index, so - like nginx - the transfer is checked against its
    declared `Content-Length` over TLS and the sha256 is computed locally and
    recorded, never claimed as vendor-verified. With no index to list, `devcrate
    install postgres` names an exact minor to fetch (its packaging build number
    is discovered by asking the mirror) rather than printing a catalogue. Note in
    the output: PostgreSQL 13 reached end-of-life on 2025-11-13, final 13.23.
  - `--from <zip>` installs from an EDB binaries zip already on disk, the same
    pipeline offline; `initdb` needs no network.

- **Python 3.8 is a managed toolchain.** `devcrate install python 3.8` fetches
  the embeddable amd64 zip from python.org, enables `import site`, and
  bootstraps pip; `devcrate python use 3.8` puts it on `PATH` by repointing a
  `python\current` junction.
  - **A toolchain, not a service - like the planned Node/Bun.** Python has no
    long-lived listener to start, stop, or watch on a port, so it is *not* a
    `ServiceKind`. The whole of "using" a version is which one sits on `PATH`,
    switched exactly as `php\current` is; `devcrate status` reports the active
    one as a line ("Python 3.8 (via python\current)") rather than a row in the
    service table. Versions are `python\python-<X.Y>` folders, matched on the
    digits like PHP - `3.8`, `38`, and `python-3.8` are one version.
  - **The embeddable distribution is made pip-capable.** It ships with `import
    site` commented out in its `._pth`, which means no `site-packages` and no
    pip; the install uncomments that line, then fetches the branch-specific
    `get-pip.py` (`bootstrap.pypa.io/pip/3.8/…` - the generic script targets the
    newest Python and would fail on 3.8) and runs it. pip's bootstrap is the one
    step that needs the network, so it degrades to a message rather than a
    failure when there is none: the Python is installed and usable, and pip can
    be finished later.
  - **The download is length/TLS-verified.** python.org publishes MD5 and GPG for
    these files, not sha256, so - like nginx and EDB - the transfer is checked
    against its declared length over TLS and the sha256 recorded, not claimed as
    verified. There is no version index either, so a branch (`3.8`) is resolved
    to its newest Windows build by asking python.org's archive which patch levels
    still ship an embeddable zip (3.8 ends at 3.8.10; later 3.8.x are
    source-only). An exact release (`3.8.10`) is fetched directly.
  - **Installing is not activating**, as with PHP: `install python 3.8` unpacks
    the version; `python use 3.8` makes it the one on `PATH`. Add `python\current`
    and its `Scripts\` to `PATH` once, the way `php\current` was added.
  - `--from <zip>` installs from an embeddable zip already on disk; site is
    enabled offline, and pip is bootstrapped if there is a network to fetch it.
  - Note: Python 3.8 is end-of-life (final Windows build 3.8.10, May 2021); it is
    supported here for legacy projects locked to it.

- **`devcrate install composer` downloads and installs Composer** into
  `composer\`, verified against the checksum getcomposer.org publishes beside the
  phar. `devcrate install composer` with no version lists the lines on offer -
  stable, the 2.2 LTS, preview, snapshot - each with the PHP it needs, and marks
  the one installed. A line is named by keyword (`stable`, `lts`, ...) or an
  exact version while the list still carries it; an older phar installs with
  `--from`.
  - **The catalogue is `getcomposer.org/versions`, JSON** - machine-readable
    unlike nginx's page, so it is read rather than scraped, and it lists the
    current release of each line rather than a wall of point releases.
  - **The download is checksum-verified, at full strength.** The hash is not in
    the catalogue, so it is fetched from the per-version `composer.phar.sha256sum`
    sidecar beside the phar and the transfer hashed as it streams, exactly as
    PHP's is - a mismatch discards it. This is where Composer differs from nginx,
    and the output says which you got. (The trust root is the sidecar, *not* the
    `installer.sig` first assumed: that signs the setup script, usable only
    through the PHP bootstrap, while the sidecar hashes the phar itself.)
  - **Composer is a tool, not a version, so it is not versioned.** One phar in
    `composer\composer.phar`, with `composer.bat` and a Git Bash `composer` shim
    beside it, and the `home\` / `cache\` directories `COMPOSER_HOME` and
    `COMPOSER_CACHE_DIR` point at. Each shim runs the phar with bare `php`, so
    Composer resolves through the same `php\current` on `PATH` that `php` does -
    which is why the phar lives outside the version folder and survives a switch.
    Add `composer\` to `PATH` once, the way `php\current` was added.
  - **Installing over an existing Composer updates it**, rather than refusing the
    way a version does; the previous version is read from the receipt and
    reported, and `--force` is not needed.
  - **The installed phar is run once, advisorily.** With a `php\current` to run
    it, `composer --version` is invoked and its banner printed - the proof the
    PHP on `PATH` can run this release. A failure there is a warning, never a
    reason to undo the install; with no PHP yet, the install still completes and
    says Composer needs one.

- **`devcrate install nginx <version>` downloads and installs an nginx build**
  into the prefix, beside any already there. `devcrate install nginx` with no
  version lists what nginx.org offers, labelled mainline / stable / legacy and
  with what is already installed marked. A version may be named in full
  (`1.31.3`) or as a series (`1.30`), by the same dotted-prefix rule
  `devcrate nginx use` uses - nginx.org lists one release per series, so the
  short form saves knowing today's patch level.
  - **The catalogue is nginx's download page**, parsed, because nginx publishes
    no machine-readable index of releases. It is kept to the two things the page
    has always done: an `<h4>` above each group, and the Windows build linked as
    `/download/nginx-<version>.zip`. A page it finds nothing in is an error
    rather than an empty list, so a format change reads as a format change.
  - **The checksum story is weaker than PHP's, and says so.** nginx signs its
    releases with PGP and publishes no hash, so there is nothing to verify the
    transfer against. What is checked instead is the declared `Content-Length`
    over TLS to nginx.org, which catches the failure that actually happens - a
    transfer cut short. The sha256 is computed locally, printed, and recorded in
    the receipt. The output does not claim the download was verified.
  - **Installing a version does not silently become the version that runs.**
    A prefix with no `current` yet activates the new build, since an inactive
    lone build would do nothing at all; a prefix that already names an active
    version is left alone and reported, with the `nginx use` line to take it.
    Installing the older stable release beside the mainline one is an ordinary
    thing to do, and it must not be a downgrade nobody asked for.
  - **The prefix is made startable**: `logs\` and `temp\` are created, and the
    `projects` junction if it is missing. `logs\` is the load-bearing one - nginx
    opens `logs/error.log` before it creates any path in its configuration, so a
    prefix without that directory fails at startup with no server.
    `conf\nginx.conf` is deliberately *not* generated: it is tracked in the
    repository and belongs to the stack, and seeding it from the vendor's
    default would produce a working nginx that includes no `sites\` and serves
    none of your vhosts. A missing one is a warning naming the cause.
  - **The new build is asked to parse the stack's configuration** (`nginx -t`)
    and its verdict reported. Advisory, never fatal: on a stack with no
    certificates yet it fails on `ssl_certificate` for reasons that have nothing
    to do with the install. What it catches is the reason worth catching early -
    a newer nginx that no longer accepts a directive the existing vhosts use.
  - **A stack still in the pre-restructure layout is refused**, before the
    network call, with the `nginx migrate` sequence that fixes it. Installing a
    second build where the versioned folder *is* the prefix would nest one
    version inside another.
  - `--from <zip>` installs from an archive already on disk, the same as for
    PHP, with the version read from the vendor's file name (`nginx-1.31.3.zip`).
  - Not delivered: uninstall, and mariadb, rabbitmq, erlang, and composer.
- **`devcrate install php <version>` downloads, verifies, and installs a PHP
  release** - the second half of roadmap item 2 for PHP, completing it end to
  end. `devcrate install php` with no version lists what windows.php.net
  offers, with what is already installed marked.
  - **The catalogue is the vendor's own `releases.json`**, which lists the
    current release of every branch - EOL branches included, so 7.4 is on it.
    Only each branch's current release is offered; superseded patch releases
    move to the vendor's `archives/` and still install with `--from`. The
    thread-safe x64 zip is picked by the `ts-`/`-x64` around the compiler tag,
    never the tag itself, which changes across branches (vc15/vs16/vs17).
  - **Every download is verified before it installs.** `releases.json`
    publishes a sha256 per zip; the transfer is hashed as it streams and a
    mismatch discards it. The file lands in `_downloads\` (gitignored) under a
    `.part` name and is only renamed once the hash matches, so a file there
    under its final name is always a verified one - and it is kept, so a
    repeat install of the same release re-hashes the cached copy and skips the
    network entirely.
  - **Refusals come before the transfer**: an already-installed version
    without `--force`, or a version the feed does not list, fails after the
    25 kB catalogue fetch, not after the 30 MB archive.
  - The install receipt now records the source archive's sha256, computed
    locally, so `--from` installs get one too.
  - HTTP is `ureq` with `rustls` - blocking like the rest of the binary, with
    certificate roots baked in rather than read from the machine's store.
  - Nothing exits 3 any more: every declared command is now built, so the
    "declared but not implemented" exit code has no remaining users. Scripts
    that check for it lose nothing.
  - Not delivered: uninstall, every runtime other than PHP, and an offline
    version *listing* (a cached archive installs offline; the catalogue still
    needs the network).
- **`devcrate install php --from <archive>`** - installs a PHP version from a
  zip already on disk: the offline fallback, and the same pipeline the
  download hands its verified file to.
  - **The version names the folder**, read from the vendor's own file name -
    `php-8.4.3-Win32-vs17-x64.zip` becomes `php\php-8.4\` on FastCGI port 9084,
    by the same `90` + digits convention everything else uses. A renamed archive
    with no readable version is an error rather than a guess; naming the version
    on the command line overrides it, in any of the usual spellings.
  - **A non-thread-safe build is refused**, read from the unpacked files
    (`php8ts.dll`) rather than the file name - the name is a poor signal in both
    directions, since the NTS download is the one marked `nts` and the TS one
    carries no marker at all. The stack runs `php-cgi.exe` as a long-lived
    FastCGI listener with `PHP_FCGI_CHILDREN`, which needs the TS build.
  - **`php.ini` is generated from the release's own `php.ini-development`**, so
    every vendor comment and default survives and the result matches the
    `php\php-8.5\php.ini` already in the repo: relative `extension_dir = "ext"`,
    `error_log = php_errors.log`, and twelve extensions on (curl, exif,
    fileinfo, gd, intl, mbstring, openssl, pdo_mysql, pdo_sqlite, sodium,
    sqlite3, zip). Lines are matched on the key *and* the value, because the
    template comments the same key more than once with different values -
    uncommenting on the key alone would enable whichever came last. Any of the
    twelve the template has no line for is reported, not skipped silently.
  - **Nothing half-installed is ever visible.** The archive is unpacked into
    `php\.devcrate-staging-php-<X.Y>` and renamed into place only once it has
    been checked and configured; the leading dot keeps it out of the `php*` scan
    that discovers versions, which would otherwise show a half-extracted folder
    in `status`, `php list`, and the dashboard the instant it appeared. Any
    failure clears the staging directory. `--force` moves the previous version
    aside rather than deleting it, and puts it back if the swap fails.
  - **An archive cannot write outside its destination.** An absolute path, a
    drive letter, a colon, or a `..` in any entry fails the whole install rather
    than being sanitised. Beyond the obvious reason, a `..` reaching a document
    root would produce a PHP that answers nothing but "No input file specified".
  - **A missing Visual C++ runtime is warned about at install time**, with the
    installer link - its absence is the single most common cause of
    `php-cgi.exe` exiting with no output at all.
  - The installed version needs nothing else edited: discovery already scans
    `php\`, so it appears in `status`, `php list`, `site add --php`, and the
    dashboard with no config written. An install receipt
    (`.devcrate-install.toml`) is left in the version directory, informational
    only - nothing reads it back, which is what keeps unpacking a folder by hand
    a complete way to install a version.
  - Naming a runtime that is planned but not installable (`nginx`, `mariadb`,
    `rabbitmq`, `erlang`, `composer`) says it is not built and points at
    `docs/installation.md`; naming a runtime that does not exist reads
    differently, so a typo is not mistaken for a missing feature.
  - Documented in `docs/cli.md`.
- **The dashboard** - `devcrate` with no arguments opens an interactive terminal
  UI built with [ratatui](https://ratatui.rs/), completing roadmap item 1. Three
  panes: the live service table, the vhosts, and a log tail.
  - **A crashed service is reported as crashed, not stopped.** This is the whole
    reason the dashboard exists: a command runs once and can only describe the
    present, so `devcrate status` has no way to tell a service that was never
    started from one that died thirty seconds ago. The dashboard stays resident
    and holds the previous scan, so a transition from `up` to `stopped` that
    nobody asked for is reported as `crashed`. A service the *user* stopped is
    not, and neither is one taken down by an action in flight.
  - Start / stop / restart per service (`s` / `x` / `t`) and for the whole stack
    (`S` / `X` / `T`), in the same dependency order, with the same graceful
    commands and the same path-scoped process matching as the subcommands. Only
    one action runs at a time; quitting mid-action asks first.
  - Vhosts: create, change PHP version, delete (with a confirmation that says
    the project folder and certificate are kept). The PHP switcher repoints
    `php\current`.
  - Log viewer: the file list is discovered rather than fixed, error logs sort
    ahead of access logs, and only the last 256 kB of a file is read so a
    month-old nginx access log opens instantly. Follow mode, scrollback,
    `Home`/`End`.
  - Three background threads - a scanner on a two-second timer, an actor that
    runs one action at a time, and a log tailer - so the drawing thread never
    blocks. It has to be this way: a TCP probe of a *dead* port costs the full
    250 ms timeout, around two seconds with the stack down, which is far too
    long to spend inside a redraw.
  - The alternate screen and raw mode are restored on every exit path including
    a panic, where the hook restores the terminal before printing. With stdout
    redirected the dashboard refuses to start and points at `status --json`
    rather than writing escape sequences into a file.
  - Documented in `docs/tui.md`.
- **One core behind both front ends.** Every action is now a function returning
  a structured result, with the printing outside it - `control::run_start`
  beside `control::start`, `site::create` beside `site::add`. The subcommand
  prints and the dashboard renders; neither reimplements the other, and nothing
  the dashboard calls can write to the terminal underneath it.
- **The `devcrate` binary** (`devcrate/`) - a Rust CLI, and the first step of
  the roadmap's sequencing item 1. It is **read-only**: it reports on the stack
  and changes nothing.
  - `devcrate status` (`--json` for scripts) - per service: installed or not,
    running or not, which ports answer, how long it has been up, and the PIDs.
    Distinguishes a service that is *running* from a port held by *something
    else*, by matching the running process's executable path against the stack
    root rather than its image name. PIDs are listed supervisor first - the
    process whose parent is not itself a match - so the `php-cgi.exe` that
    forked the FastCGI pool, the nginx master, and RabbitMQ's `erl.exe` lead
    their groups, and the uptime shown is the leader's rather than a recycled
    worker's. A port held by something that is not ours is attributed to it by
    name and PID, read from the kernel's TCP table (`GetExtendedTcpTable`); the
    same name appears in `devcrate start`'s preflight failure, so "port 9074 is
    taken" says by what.
  - `devcrate config show` / `config path` - prints the resolved configuration
    as TOML, so the values currently discovered from the folder layout can be
    pinned into a `devcrate.toml`.
  - `devcrate php list` - installed versions, their FastCGI ports, and which
    one `php\current` resolves to. `devcrate site list` - the vhosts with their
    `root` and FastCGI port.
  - `devcrate start [service]` / `restart` - starts in dependency order
    (MariaDB, PHP, RabbitMQ, nginx last), waiting for each port to actually
    answer instead of `start.bat`'s flat two-second pause, and reporting how
    long each took. Preflights every port first: one already answering without
    a process of ours behind it is reported and skipped rather than started
    into a bind failure. An already-running service is left alone, so it is
    safe to run twice. Keeps the details that matter - each PHP worker's own
    directory as its working directory (relative `extension_dir` /
    `error_log`), `PHP_FCGI_CHILDREN` / `PHP_FCGI_MAX_REQUESTS`, the
    `nginx\projects` junction self-heal, and RabbitMQ's `ERLANG_HOME` /
    `RABBITMQ_BASE`. Unlike `start.bat`, `devcrate start > log.txt` returns as
    soon as the stack is up: the script's `start /B` leaks the redirected
    stdout handle to every child, so the pipe stays open until MariaDB or the
    Erlang node exits.
  - `devcrate stop [service]` - the first command that acts on the stack. Same
    shutdown order as `stop.bat` (nginx, PHP, RabbitMQ, MariaDB) and the same
    graceful commands (`nginx -s quit`, `rabbitmqctl stop`,
    `mariadb-admin shutdown`), but it stops **only this stack**: processes are
    matched by executable path, where `taskkill /F /IM php-cgi.exe` would kill
    every `php-cgi.exe` on the machine. It also polls instead of waiting a flat
    four seconds, so a clean stack goes down in about a second. `devcrate stop
    php` stops every PHP version; `epmd` is cleared once the broker is down.
  - `devcrate php use <version>` - what `phpuse.bat` does: repoint the
    `php\current` junction that sits on `PATH`. The version can be spelled
    `8.5`, `85`, or `php-8.5`. Refuses if `php\current` is a real directory
    rather than a junction, instead of deleting it.
  - `devcrate site add <host> [--php 8.5]` / `site remove <host>` - what
    `new-vhost.bat` does: scaffold `projects\<host>\public` with an `index.php`
    stub, write the vhost conf with the same prefix-relative paths, self-heal
    the `projects` junction, and reload nginx. The PHP version comes from what
    is actually installed rather than a hard-coded port map, and defaults to the
    CLI version. It will not overwrite an existing conf without `--force`, and
    it runs `nginx -t` before reloading - one broken conf otherwise fails the
    reload for every vhost with no indication why. Third-level domains are told
    they need their own wildcard certificate. `site remove` deletes only the
    conf; the project folder and the certificate are left alone.
  - `devcrate site set-php <host> <version>` - point an existing vhost at
    another PHP version. The only command that edits a conf rather than writing
    or deleting one, so it edits as little as it can: the `fastcgi_pass` port
    and the generated `# PHP :` header comment. Every other line is copied
    through byte for byte, including anything added by hand since the file was
    generated. A `fastcgi_pass` aimed anywhere other than `127.0.0.1:` is left
    alone, and a conf with none at all is an error rather than a silent no-op.
    `nginx -t` runs before the reload.
  - **Terminal-aware output.** Colour is emitted only when stdout is a terminal
    that can render it - `NO_COLOR` turns it off, `CLICOLOR_FORCE` keeps it on
    through a pipe, and a Windows console has its ANSI interpreter enabled
    first - so `devcrate status > report.txt` contains no escape sequences. The
    terminal width is measured rather than assumed: where it cannot be
    determined, nothing is wrapped or truncated to a guess.
  - `install` was declared but exited 3 at this point; it installs PHP from a
    local archive now, as described above. The hosts-file entry remains manual,
    as with `new-vhost.bat`.
  - Stack root resolved from `--root`, then `DEVCRATE_HOME`, then the
    executable's folder, then the working directory - the last two searching
    upward, so it works from anywhere inside the tree.
  - `devcrate.toml` is optional and so is every key in it; anything missing is
    discovered from the layout (the `nginx-*` directory, one PHP entry per
    `php\php-*` folder, FastCGI port `90` + version digits).
  - Documented in `docs/cli.md`, linked from the README and the docs index.
- **MIT license** (`LICENSE`), with a License section in the README.
- **`docs/roadmap.md`** - planned development: a Rust/ratatui TUI shipped as a
  single executable, an in-program downloader/installer for PHP, Nginx,
  MariaDB, RabbitMQ, Erlang, and Composer, a cross-terminal binary with
  scriptable subcommands, one-step setup for an existing project (vhost +
  `hosts` entry + mkcert TLS), and Node.js/Bun as managed runtimes. Summarized
  in the README and linked from the docs index.

### Fixed

- **A second unpacked nginx could have committed its binary and its TLS private
  keys.** The `.gitignore` rules for nginx spelled out `nginx-1.31.1` in all
  four of them, so a `nginx-1.32\` directory matched none: both
  `nginx-1.32\nginx.exe` and `nginx-1.32\conf\certs\*-key.pem` were trackable,
  against the repo's own rule that neither is ever committed. The rules no
  longer name a version, matching the `php\` rules, which have always been
  version-agnostic. Verified both ways - binaries, logs, temp files, and
  certificate directories are ignored under the prefix and under any leftover
  `nginx-*\`, while `nginx\conf\nginx.conf` and `nginx\conf\sites\*.conf` stay
  tracked. (The prefix restructure above then moved where those tracked files
  live; the rules were rewritten with it.)
  - Also added `*-key.pem` as a repository-wide rule. mkcert names every private
    key it issues that way, its own CA included, so a certificate directory that
    is moved or added somewhere the nginx paths do not reach is still covered.
  - Found while scoping the nginx installer, which would have created exactly
    that second directory. Fixed on its own because it is a leak today,
    independent of whether the installer is ever built.
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
