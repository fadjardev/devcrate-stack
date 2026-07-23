# Installation (rebuild from a fresh clone)

This repo contains configuration only. To turn a fresh clone into a running
stack you download the binaries, extract them into place, generate the TLS
certs, and add hosts entries. Target base path throughout: `C:\devcrate`.

If you already have a working stack and only need day-to-day usage, skip to the
topic docs linked from [index.md](index.md).

## 0. Prerequisites

- Windows 10/11 x64
- Administrator access (needed once for mkcert CA install and hosts edits)
- The repo cloned to `C:\devcrate`

## 1. Visual C++ Redistributable (do this first)

PHP for Windows needs the matching VC++ runtime. If `php-cgi.exe` exits
immediately with no output, a missing redistributable is almost always why.

| PHP build | Required runtime |
| --- | --- |
| 7.4.33 (vc15) | Visual C++ 2017 x64 |
| 8.2.31 (vs16) | Visual C++ 2019 / 2022 x64 |
| 8.5.x (vs17) | Visual C++ 2022 x64 |

The VS 2022 x64 installer bundles the 2015-2022 runtimes and covers all three:

- https://aka.ms/vs/17/release/vc_redist.x64.exe

## 2. PHP (7.4, 8.2, 8.5)

Download the **Thread-Safe (TS) x64** ZIPs from
https://windows.php.net/download/ and extract each into its own folder:

| Version | Extract to | Build |
| --- | --- | --- |
| PHP 7.4.x | `C:\devcrate\php\php-7.4\` | vc15 x64 TS |
| PHP 8.2.x | `C:\devcrate\php\php-8.2\` | vs16 x64 TS |
| PHP 8.5.x | `C:\devcrate\php\php-8.5\` | vs17 x64 TS |

The `php.ini` for each version is already in the repo at
`php\<ver>\php.ini` - do not overwrite it with `php.ini-development`. Verify
each build starts:

```bat
C:\devcrate\php\php-7.4\php-cgi.exe -v
C:\devcrate\php\php-8.2\php-cgi.exe -v
C:\devcrate\php\php-8.5\php-cgi.exe -v
```

Then set up the CLI switcher (junction + PATH) - see
[php-versions.md](php-versions.md#first-time-setup).

## 3. Nginx 1.31.1

Download nginx 1.31.1 for Windows and extract so that `nginx.exe` sits at
`C:\devcrate\nginx-1.31.1\nginx.exe`, beside the `conf\` folder that is already in
the repo. Do not overwrite the versioned `conf\nginx.conf` or `conf\sites\`.

## 4. MariaDB 12.3

Download the portable (ZIP) build and extract to `C:\devcrate\mariadb\`. It is
started/stopped by `start.bat` / `stop.bat` using `mariadb\my.ini`. Initialize
the data directory per the MariaDB docs if `mariadb\data\` is empty. Details and
usage: [database.md](database.md).

## 5. RabbitMQ 4.3.2 + Erlang 27

- Erlang/OTP 27 -> `C:\devcrate\erlang\`
- RabbitMQ 4.3.2 -> `C:\devcrate\rabbitmq\`

RabbitMQ 4.3.x requires Erlang/OTP 26-27 (not 28+). Data, logs, and config are
relocated onto `E:` via `RABBITMQ_BASE`, which `start.bat` sets. Details:
[rabbitmq.md](rabbitmq.md).

## 6. mkcert + TLS certificates (Admin)

Download mkcert from https://github.com/FiloSottile/mkcert/releases, rename it
to `mkcert.exe`, and place it at `C:\devcrate\mkcert.exe`. Then, in an
**Administrator** Command Prompt, install the local CA once:

```cmd
C:\devcrate\mkcert.exe -install
```

Generate one wildcard cert per domain group into `nginx-1.31.1\conf\certs\`.
The full command set and the wildcard strategy are in
[nginx-vhosts.md](nginx-vhosts.md#tls-certificates-with-mkcert). These certs and
their private keys are intentionally not in the repo.

## 7. Hosts file (Admin)

Add the site hostnames to `C:\Windows\System32\drivers\etc\hosts` (open Notepad
as Administrator):

```
# Devcrate local stack
127.0.0.1   api-qlearning.qhomeapps.test
127.0.0.1   qlearning.qhomeapps.test
127.0.0.1   asm.qhomemart.test
127.0.0.1   supplier.qhomemart.cloud.test
127.0.0.1   hris.qhomedata.test
127.0.0.1   ic-stokdigital.test
127.0.0.1   supplier.qhomedata.id.test
```

`new-vhost.bat` prints the exact line to add for each new site. If FlyEnv (or
any tool that manages a `#X-HOSTS-BEGIN#` / `#X-HOSTS-END#` block) is installed,
put these lines outside that block so they are not overwritten.

## 8. Composer (keep it inside the stack root)

```cmd
setx COMPOSER_HOME      "C:\devcrate\composer\home"
setx COMPOSER_CACHE_DIR "C:\devcrate\composer\cache"
```

Restart open terminals afterward. Verify with `echo %COMPOSER_HOME%`.

## 9. Projects

Place each application under `C:\devcrate\projects\<domain>\`. This folder is not
versioned - clone or copy your apps in separately. Match each site's PHP version
and web root in its vhost; see [nginx-vhosts.md](nginx-vhosts.md).

## 10. Start the stack

```bat
C:\devcrate\start.bat
```

This launches MariaDB, the PHP 7.4 / 8.2 / 8.5 FastCGI listeners, RabbitMQ, and
Nginx. Confirm everything with the checklist in
[troubleshooting.md](troubleshooting.md#verification-checklist).

```bat
C:\devcrate\stop.bat
```

stops everything cleanly.

## Optional: run as a Windows service (NSSM)

> Do not auto-start at boot if `E:` is an external/removable drive - Windows may
> start the service before the drive mounts.

```cmd
nssm install DevStack "C:\devcrate\start.bat"
nssm set DevStack AppDirectory "C:\devcrate"
nssm set DevStack Description "Local PHP dev stack (Nginx + PHP FastCGI)"
nssm set DevStack Start SERVICE_DEMAND_START
nssm start DevStack
```

Remove it later with `nssm stop DevStack` then `nssm remove DevStack confirm`.
