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

- [x] List available versions per runtime, resolved from upstream at runtime rather
  than from a list baked into the binary — *PHP only. `devcrate install php` with
  no version prints the list, with what is already installed marked. The feed is
  `releases.json`, which carries every branch — EOL included — so `/archives/`
  turned out not to be needed; only each branch's current release is offered, and
  older builds install with `--from`.*
- [ ] Cache the catalogue so the UI stays responsive offline — *not built. The
  archive cache in `_downloads\` makes a repeat install work offline, but listing
  versions still needs the network.*

| Runtime    | Source                                                                |
| ---------- | --------------------------------------------------------------------- |
| PHP        | **built** — `releases.json` on `windows.php.net/downloads/releases/`  |
| Nginx      | `nginx.org/en/download.html` (Windows zips)                           |
| MariaDB    | MariaDB downloads REST API                                            |
| RabbitMQ   | GitHub releases (`rabbitmq/rabbitmq-server`)                          |
| Erlang/OTP | GitHub releases (`erlang/otp`) — Windows installer / portable         |
| Composer   | `getcomposer.org/download/` (+ `installer.sig`)                       |

### Install

- [x] Pick a version and download it with a progress bar — *PHP only.
  `devcrate install php 8.4` downloads into `_downloads\` (gitignored, and the
  archive is kept as the offline fallback). Both download and extraction report
  progress, redrawn in place on a terminal and suppressed when the output is
  redirected.*
- [x] Verify the checksum/signature the vendor publishes **before** extracting —
  *PHP only. `releases.json` publishes a sha256 per zip; the transfer is hashed
  as it streams, a mismatch discards it, and the file only gets its final name
  in `_downloads\` after the hash matches. A `--from` archive is hashed into the
  receipt but not judged — it may legitimately be a release the feed no longer
  lists.*
- [x] Extract into the standard layout (`php\php-8.5\`, `nginx-1.31.1\`, `mariadb\`, …)
  — *PHP only. The extractor already strips a wrapper directory, which is what
  nginx's zip needs, but no other runtime is wired up.*
- [x] Generate first-run config: a `php.ini` seeded from `php.ini-development` with
  the extension set Devcrate expects (curl, exif, fileinfo, gd, intl, mbstring,
  openssl, pdo_mysql, pdo_sqlite, sodium, sqlite3, zip) — *done, and it reports
  any of the twelve the template had no line for rather than skipping it
  silently.* `my.ini` — *not built, MariaDB is not installable.* The FastCGI
  port — *needed no work: it is derived from the folder name (`90` + digits),
  so installing `php-8.4` gets 9084 for free.*

### Multiple PHP versions side by side

- [x] Installing 8.5 must not disturb 7.4 or 8.2 — *each version is its own
  folder, and an existing one is refused without `--force`.*
- [x] Register the new FastCGI port (`90` + version digits) and offer the version in
  the vhost editor and in `php use` — *needed no work: discovery scans `php\`,
  so a newly installed version appears in `status`, `php list`, `site add
  --php`, and the dashboard with nothing written to any config.*

### Uninstall

- [ ] Remove a version — *not built; still `rmdir`.*
- [ ] Refuse (or warn loudly) if a vhost still points at its FastCGI port

### Preflight

- [x] Detect a missing Visual C++ redistributable — the single most common cause of
  `php-cgi.exe` exiting silently — and link the installer instead of letting the
  stack fail mysteriously — *warns at install time and links
  `aka.ms/vs/17/release/vc_redist.x64.exe`. It checks for `vcruntime140.dll` in
  the system directory, so it catches "nothing installed" rather than "the wrong
  version installed", which is the case that actually happens.*

### Not in the original scope, added because the pipeline needed it

- [x] **Refuse a non-thread-safe build.** Read from the unpacked files
  (`php8ts.dll`), not the file name — the name is a poor signal in both
  directions, since the NTS download is the one marked `nts` and the TS one
  carries no marker at all. This is the trap the motivation section describes.
- [x] **Refuse an archive that would write outside its destination.** An
  absolute path, a drive letter, a colon, or a `..` in any entry fails the whole
  install. Beyond the obvious reason, a `..` reaching a document root produces a
  PHP that answers nothing but "No input file specified".
- [x] **Never expose a half-installed version.** Unpacking happens in
  `php\.devcrate-staging-php-<X.Y>`, renamed into place only once checked and
  configured. The leading dot keeps it out of the `php*` scan that discovers
  versions. Replacing with `--force` moves the old version aside and puts it
  back if the swap fails.
- [x] **Leave an install receipt** (`.devcrate-install.toml`) recording the
  release, thread-safety, source archive, and its sha256 (computed locally, so
  a `--from` install gets one too). Informational only — nothing reads it back,
  because discovery works from the folder name, which is what keeps unpacking a
  folder by hand a complete way to install a version.

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

- ~~Do we upgrade in place, or install alongside and let the user switch?~~
  **Settled for PHP: alongside.** Each version is its own folder, so installing
  8.4 cannot disturb 8.2. Replacing the *same* version needs `--force`, and the
  old copy is moved aside rather than deleted so a failed swap can put it back.
  Still open for Nginx and Composer, where in-place is probably right — and
  Nginx has a wrinkle of its own, below.
- Where do checksums come from for runtimes that do not publish them in a machine-
  readable form? **Pinning known-good hashes in the repo is rejected** — it does
  not scale across PHP's release history and goes stale the day a version ships.
  **Settled for PHP**: `releases.json` publishes a sha256 per zip, fetched over
  the same TLS origin as the archive itself, which is exactly the intended
  answer. Pin only where there is a real trust root to pin (Composer's
  `installer.sig`). What the other vendors actually publish, in what
  machine-readable form, still needs checking one by one before it is written
  down as fact.
- GitHub API rate limits for the unauthenticated release lookups (RabbitMQ, Erlang).
- **New:** the nginx directory is versioned (`nginx-1.31.1`), and discovery picks
  the highest-sorting `nginx-*` folder. Installing a second version would create
  a second folder and silently change which one the stack uses. PHP avoids this
  entirely because its versions are meant to coexist and `php\current` names the
  active one. Decide before building the nginx installer: keep versioned folders
  and move the prefix with them, or adopt a `nginx\current` junction like PHP's.

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

  *Met.* The command downloads the release, verifies its sha256 against the
  vendor's feed, and installs it; the version is served on the right port with
  nothing else edited.
- A fresh clone can reach a running stack without visiting any vendor website

  *Not met — PHP no longer needs a vendor site, but nginx, MariaDB,
  RabbitMQ/Erlang, mkcert, and the VC++ redistributable still do.* This is the
  measure the remaining runtimes are built against, and the nginx layout
  question above is what gates the next one.