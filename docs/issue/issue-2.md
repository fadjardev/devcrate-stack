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
  than from a list baked into the binary — *PHP and nginx. `devcrate install php`
  or `install nginx` with no version prints the list, with what is already
  installed marked. PHP's feed is `releases.json`, which carries every branch —
  EOL included — so `/archives/` turned out not to be needed; only each branch's
  current release is offered, and older builds install with `--from`. nginx has
  no feed at all, so its download page is parsed for the `<h4>` above each group
  and the `/download/nginx-<version>.zip` links; a page yielding nothing is an
  error rather than an empty list.*
- [ ] Cache the catalogue so the UI stays responsive offline — *not built. The
  archive cache in `_downloads\` makes a repeat install work offline, but listing
  versions still needs the network, for both runtimes.*

| Runtime    | Source                                                                |
| ---------- | --------------------------------------------------------------------- |
| PHP        | **built** — `releases.json` on `windows.php.net/downloads/releases/`  |
| Nginx      | **built** — `nginx.org/en/download.html`, parsed (no machine-readable index) |
| MariaDB    | MariaDB downloads REST API                                            |
| RabbitMQ   | GitHub releases (`rabbitmq/rabbitmq-server`)                          |
| Erlang/OTP | GitHub releases (`erlang/otp`) — Windows installer / portable         |
| Composer   | `getcomposer.org/download/` (+ `installer.sig`)                       |

### Install

- [x] Pick a version and download it with a progress bar — *PHP and nginx.
  Both download into `_downloads\` (gitignored, and the archive is kept as the
  offline fallback). Download and extraction report progress, redrawn in place
  on a terminal and suppressed when the output is redirected.*
- [x] Verify the checksum/signature the vendor publishes **before** extracting —
  *PHP: `releases.json` publishes a sha256 per zip; the transfer is hashed as it
  streams, a mismatch discards it, and the file only gets its final name in
  `_downloads\` after the hash matches. **nginx: there is nothing to verify
  against** — it publishes only PGP signatures, so the transfer is checked
  against its declared `Content-Length` over TLS to nginx.org, and the sha256 is
  computed locally, printed, and recorded. The command says which of the two you
  got rather than implying they are the same. A `--from` archive is hashed into
  the receipt but not judged — it may legitimately be a release the vendor no
  longer lists.*
- [x] Extract into the standard layout (`php\php-8.5\`, `nginx\`, `mariadb\`, …)
  — *PHP and nginx. The extractor strips a wrapper directory when every entry is
  under one, which is exactly the difference between the two archives: nginx
  ships `nginx-1.31.3/…`, PHP ships its files at the top level.*
- [x] Generate first-run config: a `php.ini` seeded from `php.ini-development` with
  the extension set Devcrate expects (curl, exif, fileinfo, gd, intl, mbstring,
  openssl, pdo_mysql, pdo_sqlite, sodium, sqlite3, zip) — *done, and it reports
  any of the twelve the template had no line for rather than skipping it
  silently.* nginx — *nothing to generate, and deliberately so: `conf\nginx.conf`
  is tracked and belongs to the stack, not to a build. What the installer does
  instead is make the prefix startable — `logs\`, `temp\`, and the `projects`
  junction — and warn if the tracked config is missing rather than seeding a
  vendor default that would serve no vhosts.* `my.ini` — *not built, MariaDB is
  not installable.* The FastCGI port — *needed no work: it is derived from the
  folder name (`90` + digits), so installing `php-8.4` gets 9084 for free.*

### Multiple PHP versions side by side

- [x] Installing 8.5 must not disturb 7.4 or 8.2 — *each version is its own
  folder, and an existing one is refused without `--force`.*
- [x] Register the new FastCGI port (`90` + version digits) and offer the version in
  the vhost editor and in `php use` — *needed no work: discovery scans `php\`,
  so a newly installed version appears in `status`, `php list`, `site add
  --php`, and the dashboard with nothing written to any config.*

### Multiple nginx builds side by side

- [x] Installing a build must not disturb the one that runs — *`nginx\current`
  is taken only when no other version already holds it. A fresh prefix
  activates, since an inactive lone build would do nothing; an occupied one is
  left alone and the output names the version still holding it and the
  `nginx use` line to switch. Installing the stable release beside the mainline
  one must not become a downgrade nobody asked for.*
- [x] Refuse a stack still in the pre-restructure layout — *before the network
  call. Where the versioned folder is itself the prefix, a second build would
  nest inside the first; the error carries the `nginx migrate` sequence.*
- [x] Check the new build against the existing configuration — *`nginx -t` runs
  against the prefix and its verdict is reported. Advisory, never fatal: a stack
  with no certificates yet fails it for reasons unrelated to the install. What
  it catches is a newer nginx that no longer accepts a directive the vhosts use.*

### Uninstall

- [ ] Remove a version — *not built; still `rmdir`.*
- [ ] Refuse (or warn loudly) if a vhost still points at its FastCGI port, or if
  the nginx build being removed is the one `current` names

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
  `.devcrate-staging-<folder>` beside where the version will land, renamed into
  place only once checked and configured. The leading dot keeps it out of both
  scans that discover versions — `php*` under `php\`, `nginx*` under the prefix.
  Replacing with `--force` moves the old version aside and puts it back if the
  swap fails.
- [x] **Leave an install receipt** (`.devcrate-install.toml`) recording the
  release, source archive, and its sha256 (computed locally, so a `--from`
  install gets one too), plus thread-safety for PHP. Informational only —
  nothing reads it back, because discovery works from the folder name, which is
  what keeps unpacking a folder by hand a complete way to install a version.

## Constraints

- **Thread-Safe (TS) x64 PHP builds only.** The stack runs `php-cgi.exe` as a
  long-lived FastCGI listener with `PHP_FCGI_CHILDREN`; see
  [docs/php-versions.md](docs/php-versions.md).
- **Verify before extract.** Nothing is executed straight from the network, and no
  runtime is ever installed outside the stack root. Met in full for PHP; met only
  as far as the vendor allows for nginx, which publishes no hash — see the open
  question below.
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
  `installer.sig`).

  **Answered for nginx, and the answer is "it doesn't".** nginx publishes a
  `.asc` PGP signature beside each zip and nothing else — no sha256, no
  machine-readable index of any kind. So the install checks what remains: TLS to
  nginx.org, and the `Content-Length` the server declared, which catches a
  truncated transfer. The sha256 is computed locally, printed, and recorded in
  the receipt so it can be compared by hand. Verifying the signature properly
  means shipping and trusting nginx's signing keys, which is its own feature and
  is **not built**. This is said in the command's output, not just here: the
  difference between "verified" and "downloaded over TLS" is exactly the kind of
  thing that must not be quietly rounded up.

  What MariaDB, RabbitMQ, and Erlang publish still needs checking one by one
  before it is written down as fact.
- GitHub API rate limits for the unauthenticated release lookups (RabbitMQ, Erlang).
- ~~The nginx directory is versioned (`nginx-1.31.1`), and discovery picks the
  highest-sorting `nginx-*` folder, so installing a second version would
  silently change which one the stack uses.~~ **Settled: one stable prefix,
  versions inside it, `nginx\current` naming the active one** — the PHP model,
  adapted. Done ahead of the nginx installer, since it decides where an
  installed nginx has to land.

  The adaptation matters. PHP's config is genuinely *per-version* (each
  `php.ini` differs, and three workers run at once), so a version is a
  self-contained folder. nginx's config is *per-stack*: the vhosts, the
  certificates, and the logs belong to the stack, and only one nginx runs. So
  the prefix holds all of that and the builds sit inside it — the mirror image
  of PHP, for the same reason.

  The payoff is that no vhost conf needed rewriting, because nginx's two path
  bases both stay put: `root` and the logs resolve against the prefix,
  `ssl_certificate` against the conf directory, and neither is inside the
  versioned folder. `devcrate nginx migrate` moves an older stack across, and
  both layouts are still detected so an unmigrated one keeps running.

  This model will **not** transfer to MariaDB or RabbitMQ: their on-disk data
  formats are version-specific, so switching versions under one data directory
  is a migration, not a junction rewrite. Composer and mkcert are single
  binaries and want in-place replacement instead.

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

  *Not met — PHP and nginx no longer need a vendor site, but MariaDB,
  RabbitMQ/Erlang, mkcert, and the VC++ redistributable still do.* This is the
  measure the remaining runtimes are built against. Worth noting how much of the
  stack the two that are done cover: nginx plus one PHP version is a stack that
  serves, which is the half a fresh clone needs first.