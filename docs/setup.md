# Devcrate — Setup Guide

Linear walkthrough to get the stack running. For deeper reference on any topic,
see the [docs/](index.md) directory.

- **Stack:** Nginx 1.31.1 - PHP 7.4 / 8.2 / 8.5 - MariaDB 12.3 -
  RabbitMQ 4.3.2 (Erlang 27) - mkcert - Windows 10/11 x64
- **Stack root:** any folder you like — the scripts resolve their own location
  and the nginx configs are relative. Examples below use `C:\devcrate\`;
  substitute your own path.

This repo tracks configuration only. Binaries, runtime data, TLS keys, and
`projects/` are not committed - you download and generate them locally. Full
detail for each step lives in [installation.md](installation.md).

---

## 1. Install the Visual C++ Redistributable (first!)

PHP for Windows needs the matching VC++ runtime. If `php-cgi.exe` exits
immediately with no output, this is almost always why.

| PHP build | Required runtime |
| --- | --- |
| 7.4.33 (vc15) | Visual C++ 2017 x64 |
| 8.2.31 (vs16) | Visual C++ 2019 / 2022 x64 |
| 8.5.x (vs17) | Visual C++ 2022 x64 |

Installer that covers all three: <https://aka.ms/vs/17/release/vc_redist.x64.exe>

---

## 2. PHP 7.4 / 8.2 / 8.5

Download the **Thread-Safe (TS) x64** ZIPs from
<https://windows.php.net/download/> and extract:

| Version | Extract to | Build |
| --- | --- | --- |
| PHP 7.4.x | `C:\devcrate\php\php74\` | vc15 x64 TS |
| PHP 8.2.x | `C:\devcrate\php\php82\` | vs16 x64 TS |
| PHP 8.5.x | `C:\devcrate\php\php85\` | vs17 x64 TS |

The `php.ini` for each version is already in the repo - don't overwrite it.
Verify each build:

```cmd
C:\devcrate\php\php74\php-cgi.exe -v
C:\devcrate\php\php82\php-cgi.exe -v
C:\devcrate\php\php85\php-cgi.exe -v
```

**CLI version switching.** `php` on the command line resolves through the
`php\current` junction. Create it by running the switcher once:

```bat
C:\devcrate\phpuse.bat 85
```

Add `C:\devcrate\php\current` and `C:\devcrate` to your **user** PATH (with your
actual stack root), open a new terminal, then switch anytime with `phpuse 85` /
`phpuse 82` / `phpuse 74`. Details: [php-versions.md](php-versions.md).

---

## 3. Nginx, MariaDB, RabbitMQ

- **Nginx 1.31.1** -> extract so `nginx.exe` sits beside the tracked
  `nginx-1.31.1\conf\`.
- **MariaDB 12.3** -> `C:\devcrate\mariadb\` (usage: [database.md](database.md)).
- **RabbitMQ 4.3.2 + Erlang 27** -> `C:\devcrate\rabbitmq\` and `C:\devcrate\erlang\`
  (usage: [rabbitmq.md](rabbitmq.md)).

---

## 4. mkcert local CA + TLS certificates (Admin)

Place mkcert at `C:\devcrate\mkcert.exe`, then in an **Administrator** CMD install the
CA once:

```cmd
C:\devcrate\mkcert.exe -install
```

Generate one wildcard cert per domain group into `nginx-1.31.1\conf\certs\`. The
full command set and the wildcard strategy are in
[nginx-vhosts.md](nginx-vhosts.md#tls-certificates-with-mkcert). These
`.pem` files are not versioned - regenerate them after cloning.

---

## 5. Hosts file (Admin)

Add the site hostnames to `C:\Windows\System32\drivers\etc\hosts`:

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

`new-vhost.bat` prints the exact line for each new site. If FlyEnv manages a
`#X-HOSTS-BEGIN#` / `#X-HOSTS-END#` block, add these lines outside it.

---

## 6. Composer (keep it inside the stack root)

```cmd
setx COMPOSER_HOME      "C:\devcrate\composer\home"
setx COMPOSER_CACHE_DIR "C:\devcrate\composer\cache"
```

Restart open terminals afterward.

---

## 7. Start / stop the stack

```cmd
C:\devcrate\start.bat    REM MariaDB + PHP 7.4/8.2/8.5 FastCGI + RabbitMQ + Nginx
C:\devcrate\stop.bat     REM graceful shutdown of everything
```

**Ports:**

| Service | Address |
| --- | --- |
| MariaDB | `127.0.0.1:3306` |
| PHP 7.4 FastCGI | `127.0.0.1:9074` |
| PHP 8.2 FastCGI | `127.0.0.1:9082` |
| PHP 8.5 FastCGI | `127.0.0.1:9085` |
| RabbitMQ AMQP | `127.0.0.1:5672` |
| RabbitMQ Mgmt UI | `127.0.0.1:15672` |
| Nginx HTTP / HTTPS | `:80` / `:443` |

---

## 8. Add a project

```cmd
C:\devcrate\new-vhost.bat myapp.test php85
```

Scaffolds `projects\myapp.test\public\`, writes the vhost conf, and reloads
nginx. Adjust the web root and cert for the framework as needed, then add the
hosts entry it prints. Full guide (frameworks, web roots, third-level domain
certs): [nginx-vhosts.md](nginx-vhosts.md).

---

## 9. Verify

Run through the checklist in
[troubleshooting.md](troubleshooting.md#verification-checklist). If a
site fails, check `nginx-1.31.1\logs\<domain>.error.log`.

---

## More documentation

| Topic | Doc |
| --- | --- |
| How it all fits together | [architecture.md](architecture.md) |
| Full install reference | [installation.md](installation.md) |
| PHP versions + `phpuse` | [php-versions.md](php-versions.md) |
| Nginx vhosts + TLS | [nginx-vhosts.md](nginx-vhosts.md) |
| MariaDB | [database.md](database.md) |
| RabbitMQ | [rabbitmq.md](rabbitmq.md) |
| Troubleshooting | [troubleshooting.md](troubleshooting.md) |
