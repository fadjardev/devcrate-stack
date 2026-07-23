# PHP versions

The stack runs three PHP versions at once. There are two independent notions of
"which PHP":

- **CLI** - what `php`, `composer`, `laravel` resolve to in a terminal. Global,
  switched with `phpuse`.
- **Web (FastCGI)** - what each nginx vhost proxies to. Decided per site by the
  `fastcgi_pass` port. See [nginx-vhosts.md](nginx-vhosts.md).

| Version | Folder | FastCGI port | Windows build |
| --- | --- | --- | --- |
| PHP 7.4.33 | `php\php-7.4\` | 9074 | vc15 x64 TS |
| PHP 8.2.31 | `php\php-8.2\` | 9082 | vs16 x64 TS |
| PHP 8.5.x | `php\php-8.5\` | 9085 | vs17 x64 TS |

## CLI version switching (`phpuse`)

`php` on the CLI resolves through a junction at `C:\devcrate\php\current`. Your user
`PATH` contains `C:\devcrate\php\current` (a stable path), so re-pointing the junction
instantly changes which PHP the `php` command runs - no PATH edits, no new
terminal needed.

```bat
phpuse            REM show active version + list installed versions
phpuse 8.5        REM switch global CLI PHP to 8.5
phpuse 8.2        REM switch to 8.2
phpuse 7.4        REM switch to 7.4
```

The version is matched on its digits, so `8.5`, `85`, and `php-8.5` all name the
same folder. `devcrate php use 8.5` does the same thing and accepts the same
spellings — see [cli.md](cli.md).

`C:\devcrate` is also on `PATH`, so `phpuse` and the other `.bat` helpers are callable
from anywhere.

### How it works

`phpuse.bat` deletes and recreates the `php\current` junction to point at the
requested `php\<ver>` folder:

```
C:\devcrate\php\current   --junction-->   C:\devcrate\php\php-8.5
        ^ on PATH                            ^ actual binaries
```

Because the junction resolves at file-open time, every new `php` process picks up
the change immediately.

### First-time setup

On a fresh machine, run the switcher once — it creates the junction itself:

```bat
C:\devcrate\phpuse.bat 8.5
```

Then add both of these to your **user** PATH (not system), using your actual
stack root, and open a new terminal:

```
C:\devcrate\php\current
C:\devcrate
```

(Equivalent manual form: `mklink /J <stack-root>\php\current <stack-root>\php\php-8.5`.
Junctions do not require Administrator.) These PATH entries and the junction
target are the only absolute, machine-specific paths in the whole setup — both
live outside the repo.

## php.ini

Each version has its own `php.ini` at `php\<ver>\php.ini`, and these are the only
PHP files tracked in the repo. Confirm a build is reading the right one:

```bat
C:\devcrate\php\php-8.5\php-cgi.exe -i | findstr "Loaded Configuration"
```

Extensions enabled for app work include: `curl`, `mbstring`, `openssl`,
`pdo_mysql`, `pdo_sqlite`, `sqlite3`, `fileinfo`, `intl`, `gd`, `zip`, `exif`,
`sodium`, and `sockets` (required by the RabbitMQ PHP client). List what a build
has loaded with:

```bat
C:\devcrate\php\php-8.5\php.exe -m
```

Common shared settings (timezone `Asia/Jakarta`, `upload_max_filesize=64M`) are
set in each `php.ini`.

## Which PHP does an app need?

Check `composer.json` -> `require` -> `php`:

| composer requirement | Use |
| --- | --- |
| `>=8.2`, `>=8.1` | `php-8.2` (or `php-8.5` if the app is 8.5-compatible) |
| `>=7.4` | `php-7.4` |
| none / no composer | `php-7.4` is fine |

Picking the wrong version yields a `Composer detected issues in your platform`
fatal error on first load.

## Adding a new PHP version

1. Download the Thread-Safe x64 ZIP from https://windows.php.net/download/ and
   extract to `C:\devcrate\php\php-<X.Y>\` (e.g. `php-8.4`). The `php-` prefix and
   the dotted version are what the tooling looks for.
2. Install the matching VC++ runtime (see
   [installation.md](installation.md#1-visual-c-redistributable-do-this-first)).
3. Create `php-<X.Y>\php.ini` (copy an existing one and adjust) and enable the same
   extensions.
4. To serve it over the web, add a FastCGI listener in `start.bat` on port
   `90` + the version digits (`php-8.4` -> 9084); `stop.bat` is already generic
   (`taskkill php-cgi.exe`).
5. Nothing else needs editing. `phpuse`, `new-vhost.bat`, and every `devcrate`
   subcommand discover installed versions by listing `php\php-*` and derive both
   the display name and the FastCGI port from the folder name — so a new version
   appears in `devcrate php list` and `devcrate start` as soon as it is unpacked.

## Known limitation: zip on PHP 7.4

The PHP 7.4.33 vc15 x64 build ships without `php_zip.dll`. See
[troubleshooting.md](troubleshooting.md#php-74-zip-extension-missing) to enable
it.
