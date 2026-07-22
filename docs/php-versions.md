# PHP versions

The stack runs three PHP versions at once. There are two independent notions of
"which PHP":

- **CLI** - what `php`, `composer`, `laravel` resolve to in a terminal. Global,
  switched with `phpuse`.
- **Web (FastCGI)** - what each nginx vhost proxies to. Decided per site by the
  `fastcgi_pass` port. See [nginx-vhosts.md](nginx-vhosts.md).

| Version | Folder | FastCGI port | Windows build |
| --- | --- | --- | --- |
| PHP 7.4.33 | `php\php74\` | 9074 | vc15 x64 TS |
| PHP 8.2.31 | `php\php82\` | 9082 | vs16 x64 TS |
| PHP 8.5.x | `php\php85\` | 9085 | vs17 x64 TS |

## CLI version switching (`phpuse`)

`php` on the CLI resolves through a junction at `E:\dev\php\current`. Your user
`PATH` contains `E:\dev\php\current` (a stable path), so re-pointing the junction
instantly changes which PHP the `php` command runs - no PATH edits, no new
terminal needed.

```bat
phpuse            REM show active version + list installed versions
phpuse 85         REM switch global CLI PHP to 8.5
phpuse 82         REM switch to 8.2
phpuse 74         REM switch to 7.4
```

`E:\dev` is also on `PATH`, so `phpuse` and the other `.bat` helpers are callable
from anywhere.

### How it works

`phpuse.bat` deletes and recreates the `php\current` junction to point at the
requested `php\<ver>` folder:

```
E:\dev\php\current   --junction-->   E:\dev\php\php85
        ^ on PATH                            ^ actual binaries
```

Because the junction resolves at file-open time, every new `php` process picks up
the change immediately.

### First-time setup

On a fresh machine, create the junction and put it on PATH once:

```bat
mklink /J E:\dev\php\current E:\dev\php\php85
```

Then add both of these to your **user** PATH (not system), and open a new
terminal:

```
E:\dev\php\current
E:\dev
```

`mklink /J` creates a directory junction and does not require Administrator.

## php.ini

Each version has its own `php.ini` at `php\<ver>\php.ini`, and these are the only
PHP files tracked in the repo. Confirm a build is reading the right one:

```bat
E:\dev\php\php85\php-cgi.exe -i | findstr "Loaded Configuration"
```

Extensions enabled for app work include: `curl`, `mbstring`, `openssl`,
`pdo_mysql`, `pdo_sqlite`, `sqlite3`, `fileinfo`, `intl`, `gd`, `zip`, `exif`,
`sodium`, and `sockets` (required by the RabbitMQ PHP client). List what a build
has loaded with:

```bat
E:\dev\php\php85\php.exe -m
```

Common shared settings (timezone `Asia/Jakarta`, `upload_max_filesize=64M`) are
set in each `php.ini`.

## Which PHP does an app need?

Check `composer.json` -> `require` -> `php`:

| composer requirement | Use |
| --- | --- |
| `>=8.2`, `>=8.1` | `php82` (or `php85` if the app is 8.5-compatible) |
| `>=7.4` | `php74` |
| none / no composer | `php74` is fine |

Picking the wrong version yields a `Composer detected issues in your platform`
fatal error on first load.

## Adding a new PHP version

1. Download the Thread-Safe x64 ZIP from https://windows.php.net/download/ and
   extract to `E:\dev\php\php<NN>\` (e.g. `php84`).
2. Install the matching VC++ runtime (see
   [installation.md](installation.md#1-visual-c-redistributable-do-this-first)).
3. Create `php<NN>\php.ini` (copy an existing one and adjust) and enable the same
   extensions.
4. To serve it over the web, add a FastCGI listener in `start.bat` on port
   `90<NN>` and a matching `stop.bat` is already generic (`taskkill php-cgi.exe`).
5. To use it on the CLI, `phpuse <NN>` works automatically once `php<NN>\php.exe`
   exists.

## Known limitation: zip on PHP 7.4

The PHP 7.4.33 vc15 x64 build ships without `php_zip.dll`. See
[troubleshooting.md](troubleshooting.md#php-74-zip-extension-missing) to enable
it.
