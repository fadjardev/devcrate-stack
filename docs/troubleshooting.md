# Troubleshooting

## Verification checklist

After `start.bat`, confirm:

- [ ] `http://localhost` redirects to `https://localhost` (a 404 is fine - there
      is no default vhost)
- [ ] Each site loads over HTTPS with no certificate warning:
  - `https://hris.qhomedata.test`
  - `https://asm.qhomemart.test`
  - `https://supplier.qhomemart.cloud.test`
  - `https://ic-stokdigital.test`
  - `https://api-qlearning.qhomeapps.test`
  - `https://qlearning.qhomeapps.test`
  - `https://supplier.qhomedata.id.test`
- [ ] PHP version per site matches:

  | Site | Framework | PHP |
  | --- | --- | --- |
  | `ic-stokdigital.test` | CI3 | 7.4.33 |
  | `supplier.qhomemart.cloud.test` | CI2 | 7.4.33 |
  | `supplier.qhomedata.id.test` | CI2 | 7.4.33 |
  | `asm.qhomemart.test` | CI3 | 8.2.31 |
  | `hris.qhomedata.test` | CI3 | 8.2.31 |
  | `api-qlearning.qhomeapps.test` | CI4 | 8.2.31 |
  | `qlearning.qhomeapps.test` | CI4 | 8.2.31 |

- [ ] `date.timezone` shows `Asia/Jakarta` in phpinfo
- [ ] `upload_max_filesize` shows `64M` in phpinfo
- [ ] RabbitMQ mgmt UI reachable at `http://127.0.0.1:15672` (guest/guest)
- [ ] `C:\devcrate\php\php-8.2\php.exe C:\devcrate\tools\rabbitmq-smoketest\test.php` ->
      `RESULT: OK`

## Log locations

```
Nginx errors      C:\devcrate\nginx-1.31.1\logs\error.log
Nginx per-site    C:\devcrate\nginx-1.31.1\logs\<domain>.error.log
PHP errors        C:\devcrate\php\php-7.4\php_errors.log
                  C:\devcrate\php\php-8.2\php_errors.log
                  C:\devcrate\php\php-8.5\php_errors.log
MariaDB           C:\devcrate\mariadb\mariadb_error.log
RabbitMQ          C:\devcrate\rabbitmq\data\log\rabbit@<HOSTNAME>.log
```

## Every PHP site returns 404 "No input file specified"

Vhost roots are prefix-relative (`root projects/<domain>`), resolved through the
`nginx-1.31.1\projects -> ..\projects` junction. If that junction is missing,
static requests 404 from nginx and `.php` requests 404 with PHP's
*No input file specified*. Recreate it (or just run `start.bat`, which
self-heals it):

```bat
mklink /J <stack-root>\nginx-1.31.1\projects <stack-root>\projects
```

Do **not** "fix" a vhost by changing its root to `../projects/...` — nginx will
serve static files from it, but PHP-CGI on Windows rejects any
`SCRIPT_FILENAME` containing `..` and returns exactly this 404.

## php-cgi.exe exits immediately with no output

Almost always a missing Visual C++ Redistributable for that build. Install the
VS 2022 x64 runtime (covers 2015-2022):
https://aka.ms/vs/17/release/vc_redist.x64.exe

| PHP build | Runtime |
| --- | --- |
| 7.4.33 (vc15) | VC++ 2017 x64 |
| 8.2.31 (vs16) | VC++ 2019 / 2022 x64 |
| 8.5.x (vs17) | VC++ 2022 x64 |

Then re-test: `C:\devcrate\php\php-8.5\php-cgi.exe -v`.

## `php` is not recognized on the CLI

The `php\current` junction or the PATH entry is missing. Recreate the junction
by running the switcher once:

```bat
C:\devcrate\phpuse.bat 8.5
```

Add `C:\devcrate\php\current` and `C:\devcrate` (your actual stack root) to your
user PATH, then open a new terminal. Full detail:
[php-versions.md](php-versions.md#first-time-setup).

## Composer platform error on first load

`Composer detected issues in your platform` means the site is being served by
the wrong PHP version. Check the app's `composer.json` requirement and point the
vhost's `fastcgi_pass` at the correct port
([nginx-vhosts.md](nginx-vhosts.md#anatomy-of-a-vhost)), then reload nginx.

## Certificate warning in the browser

- The mkcert local CA is not installed: run `C:\devcrate\mkcert.exe -install` in an
  Admin CMD, then restart the browser.
- Wrong cert for the domain level: a third-level host (e.g.
  `api.mygroup.test`) needs a `*.mygroup.test` cert, not `*.test`. See
  [nginx-vhosts.md](nginx-vhosts.md#tls-certificates-with-mkcert).
- Certs missing after a fresh clone: they are not versioned - regenerate them.

## Port 3306 already in use

Another MySQL/MariaDB (FlyEnv, XAMPP, etc.) holds the port and `mariadbd.exe`
fails with `Bind on TCP/IP port ... 10048`. Stop that service, or set `port` in
`mariadb\my.ini` to `3307` and update each app's DB config.

## Hosts entries keep disappearing

If FlyEnv or a similar tool manages a `#X-HOSTS-BEGIN#` / `#X-HOSTS-END#` block
in the hosts file, it can overwrite anything inside it. Put the stack's entries
outside that block, under their own `# Devcrate local stack` heading.

## RabbitMQ won't restart cleanly

`epmd.exe` (Erlang Port Mapper Daemon) stays resident after `rabbitmqctl stop`.
`stop.bat` force-kills `erl.exe` and `epmd.exe` as a fallback. If a start still
fails, kill any lingering `erl.exe` / `epmd.exe` manually and check
`rabbitmq\data\log\rabbit@<HOSTNAME>.log`. Also confirm Erlang is OTP 26-27, not
28+.

## PHP 7.4 zip extension missing

`php_zip.dll` is absent from the PHP 7.4.33 vc15 x64 build; the extension needs
`zlib1.dll` (and `bz2.dll`, `zstd.dll`) which are not bundled. To enable it:

1. Download the dependency pack from
   `https://windows.php.net/downloads/php-sdk/deps/vc15/x64/` - grab `zlib-*.zip`
   (and `bzip2-*.zip`, `zstd-*.zip` if needed).
2. Place the `.dll` files into `C:\devcrate\php\php-7.4\` (the root, not `ext\`).
3. Get `php_zip-*-7.4-nts-vc15-x64.zip` from the windows.php.net extras and put
   `php_zip.dll` into `C:\devcrate\php\php-7.4\ext\`.
4. Uncomment `extension=zip` in `C:\devcrate\php\php-7.4\php.ini`.
5. Restart the stack.

PHP 8.2 and 8.5 bundle `zip` already.
