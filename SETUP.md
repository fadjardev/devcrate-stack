# Local PHP Dev Stack â€“ Setup Guide

**Stack:** Nginx 1.31.1 Â· PHP 7.4 / 8.2 / 8.3 Â· MariaDB 12.3 Â· RabbitMQ 4.3.2 (Erlang 27) Â· mkcert Â· Windows 10 x64  
**Base path:** `E:\dev\`

---

## 1. What you already have

| Item                                                              | Status       |
| ----------------------------------------------------------------- | ------------ |
| `E:\dev\nginx-1.31.1\`                                            | âœ… Extracted |
| `E:\dev\php\php74\` (PHP 7.4.33 vc15 x64 NTS)                     | âœ… Extracted |
| `E:\dev\php\php82\` (PHP 8.2.31 vs16 x64 NTS)                     | âœ… Extracted |
| All generated config files (`nginx.conf`, vhost confs, bat files) | âœ… Generated |

---

## 2. Still to download

### PHP 8.3 (optional â€“ only if you use port 9083)

- Page: https://windows.php.net/download/
- Build to grab: **PHP 8.3.x Non-Thread-Safe (NTS) x64** â€” the VS16 x64 ZIP
- Extract to: `E:\dev\php\php83\`

### Nginx (already have 1.31.1 â€” skip)

### mkcert

- Page: https://github.com/FiloSottile/mkcert/releases
- File to grab: `mkcert-v*-windows-amd64.exe`
- Rename to `mkcert.exe` and place at: `E:\dev\mkcert.exe`

---

## 3. Visual C++ Redistributable (most common cause of PHP failing silently)

PHP on Windows requires the matching VC++ runtime DLLs.  
If `php-cgi.exe` exits immediately with no output, this is almost always why.

| PHP build       | Required runtime                               |
| --------------- | ---------------------------------------------- |
| 7.4.33 **vc15** | Visual C++ **2017** Redistributable x64        |
| 8.2.31 **vs16** | Visual C++ **2019 / 2022** Redistributable x64 |
| 8.3.x  **vs16** | Visual C++ **2019 / 2022** Redistributable x64 |

**Download (installs both â€” covers all three PHP versions):**  
https://aka.ms/vs/17/release/vc_redist.x64.exe  
*(This is the VS 2022 x64 installer; it bundles VS 2015â€“2022 runtimes.)*

Install it, then re-test `php-cgi.exe` from CMD:

```
E:\dev\php\php74\php-cgi.exe -v
E:\dev\php\php82\php-cgi.exe -v
```

---

## 4. Copy `php.ini` to each PHP folder

The generated `php.ini` files are already at the correct locations:

```
E:\dev\php\php74\php.ini   â† generated âœ…
E:\dev\php\php82\php.ini   â† generated âœ…
E:\dev\php\php83\php.ini   â† generated âœ…  (for when you add PHP 8.3)
```

Verify each PHP sees its own ini:

```
E:\dev\php\php74\php-cgi.exe -i | findstr "Loaded Configuration"
E:\dev\php\php82\php-cgi.exe -i | findstr "Loaded Configuration"
```

---

## 5. Install mkcert local CA (run once, as Administrator)

Open **Command Prompt as Administrator**, then:

```cmd
E:\dev\mkcert.exe -install
```

This installs the local CA certificate into the Windows Certificate Store  
(and Firefox's NSS store if present). Browsers will trust all certs signed  
by this CA without warnings.

---

## 6. Generate TLS certificates

Still in the **Admin CMD**, run these six commands â€” one per wildcard group:

```cmd
REM Covers: ic-stokdigital.test  and any future simple *.test domains
E:\dev\mkcert.exe ^
  -cert-file E:\dev\nginx-1.31.1\conf\certs\_wildcard.test.pem ^
  -key-file  E:\dev\nginx-1.31.1\conf\certs\_wildcard.test-key.pem ^
  "*.test"

REM Covers: api-qlearning.qhomeapps.test  qlearning.qhomeapps.test
E:\dev\mkcert.exe ^
  -cert-file E:\dev\nginx-1.31.1\conf\certs\_wildcard.qhomeapps.test.pem ^
  -key-file  E:\dev\nginx-1.31.1\conf\certs\_wildcard.qhomeapps.test-key.pem ^
  "*.qhomeapps.test"

REM Covers: asm.qhomemart.test
E:\dev\mkcert.exe ^
  -cert-file E:\dev\nginx-1.31.1\conf\certs\_wildcard.qhomemart.test.pem ^
  -key-file  E:\dev\nginx-1.31.1\conf\certs\_wildcard.qhomemart.test-key.pem ^
  "*.qhomemart.test"

REM Covers: hris.qhomedata.test
E:\dev\mkcert.exe ^
  -cert-file E:\dev\nginx-1.31.1\conf\certs\_wildcard.qhomedata.test.pem ^
  -key-file  E:\dev\nginx-1.31.1\conf\certs/_wildcard.qhomedata.test-key.pem ^
  "*.qhomedata.test"

REM Covers: supplier.qhomemart.cloud.test
E:\dev\mkcert.exe ^
  -cert-file E:\dev\nginx-1.31.1\conf\certs\_wildcard.qhomemart.cloud.test.pem ^
  -key-file  E:\dev\nginx-1.31.1\conf\certs\_wildcard.qhomemart.cloud.test-key.pem ^
  "*.qhomemart.cloud.test"

REM Covers: supplier.qhomedata.id.test
E:\dev\mkcert.exe ^
  -cert-file E:\dev\nginx-1.31.1\conf\certs\_wildcard.qhomedata.id.test.pem ^
  -key-file  E:\dev\nginx-1.31.1\conf\certs\_wildcard.qhomedata.id.test-key.pem ^
  "*.qhomedata.id.test"
```

> **Wildcard limitation:** `*.test` covers `ic-stokdigital.test` but NOT  
> `api-qlearning.qhomeapps.test` (that's a third-level domain). Each group  
> needs its own `*.group.test` wildcard. Multi-level wildcards like `*.*.test`  
> are not valid in X.509.

---

## 7. Hosts file entries (requires Admin)

Open `C:\Windows\System32\drivers\etc\hosts` in Notepad **as Administrator**  
and add these lines at the bottom:

```
# E:\dev local stack
127.0.0.1   api-qlearning.qhomeapps.test
127.0.0.1   qlearning.qhomeapps.test
127.0.0.1   asm.qhomemart.test
127.0.0.1   supplier.qhomemart.cloud.test
127.0.0.1   hris.qhomedata.test
127.0.0.1   ic-stokdigital.test
127.0.0.1   supplier.qhomedata.id.test
```

> **Note:** if FlyEnv is still installed, it auto-manages a block in this file
> between `#X-HOSTS-BEGIN#` and `#X-HOSTS-END#` markers and may overwrite
> entries placed inside it. Add the lines above **outside** that block
> (e.g. in their own `# E:\dev local stack` section) so they survive FlyEnv's
> hosts sync.

Every time you run `new-vhost.bat` it prints the exact line you need to add.

---

## 8. Composer â€“ keep it off the C: drive

Run in a normal CMD (not Admin):

```cmd
setx COMPOSER_HOME     "E:\dev\composer\home"
setx COMPOSER_CACHE_DIR "E:\dev\composer\cache"
```

Then **restart any open CMD/terminal windows** for the variables to take effect.  
Verify with: `echo %COMPOSER_HOME%`

---

## 9. Start / stop the stack

```cmd
E:\dev\start.bat    â† starts PHP 7.4, 8.2, 8.3 FastCGI + Nginx
E:\dev\stop.bat     â† graceful quit
```

**Ports in use:**

| Service         | Address        |
| --------------- | -------------- |
| MariaDB         | 127.0.0.1:3306 |
| PHP 7.4 FastCGI | 127.0.0.1:9074 |
| PHP 8.2 FastCGI | 127.0.0.1:9082 |
| PHP 8.3 FastCGI | 127.0.0.1:9083 |
| RabbitMQ AMQP   | 127.0.0.1:5672 |
| RabbitMQ Mgmt UI| 127.0.0.1:15672|
| Nginx HTTP      | 0.0.0.0:80     |
| Nginx HTTPS     | 0.0.0.0:443    |

---

## 9b. Database – MariaDB 12.3 (portable)

**Location:** `E:\dev\mariadb\`  ·  **Config:** `E:\dev\mariadb\my.ini`  ·  **Data:** `E:\dev\mariadb\data\`

MariaDB 12.3.2 (LTS) is installed as a portable build and started/stopped by
`start.bat` / `stop.bat` alongside Nginx and PHP. It is a drop-in replacement for
MySQL — the same `mysqli` / `pdo_mysql` drivers (already enabled in PHP 7.4 and
8.2) connect to it unchanged.

**Connection defaults:**

| Setting  | Value         |
| -------- | ------------- |
| Host     | `127.0.0.1`   |
| Port     | `3306`        |
| User     | `root`        |
| Password | *(none)*      |

> ⚠️ **Port 3306 conflict with FlyEnv.** If FlyEnv is still running and has its
> own MySQL/MariaDB started, it occupies port 3306 and `mariadbd.exe` will fail
> to bind (`Can't start server: Bind on TCP/IP port ... 10048`). Stop FlyEnv's
> database (or quit FlyEnv) before starting this stack, or change `port` in
> `my.ini` to e.g. `3307`.

**CLI client:**

```cmd
E:\dev\mariadb\bin\mariadb.exe -u root
```

**Set a root password (optional):**

```cmd
E:\dev\mariadb\bin\mariadb-admin.exe -u root password "yourpassword"
```

Then update each app's DB config (`username=root`, `password=yourpassword`).

**Create a database for a project:**

```cmd
E:\dev\mariadb\bin\mariadb.exe -u root -e "CREATE DATABASE myapp CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci;"
```

**Import a SQL dump:**

```cmd
E:\dev\mariadb\bin\mariadb.exe -u root myapp < C:\path\to\dump.sql
```

**App DB config per framework:**

| Framework | File                              | Keys                                                    |
| --------- | --------------------------------- | ------------------------------------------------------- |
| CI2 / CI3 | `application/config/database.php` | `hostname=127.0.0.1`, `username=root`, `password=`, `database=myapp`, `dbdriver=mysqli` |
| CI4       | `.env`                            | `database.default.hostname=127.0.0.1`, `database.default.username=root`, `database.default.password=`, `database.default.database=myapp` |
| Laravel   | `.env`                            | `DB_CONNECTION=mysql`, `DB_HOST=127.0.0.1`, `DB_PORT=3306`, `DB_USERNAME=root`, `DB_PASSWORD=`, `DB_DATABASE=myapp` |

> Use `127.0.0.1`, **not** `localhost`. On Windows, `localhost` can make the
> client try a named pipe/socket instead of TCP.

**Error log:** `E:\dev\mariadb\mariadb_error.log`

---

## 9c. Message broker – RabbitMQ 4.3.2 (portable)

**Erlang:** `E:\dev\erlang` (OTP 27.3.4.13) · **Broker:** `E:\dev\rabbitmq` · **Data/logs/config:** `E:\dev\rabbitmq\data\`

RabbitMQ 4.3.2 runs as a portable build alongside Nginx/PHP/MariaDB, started and
stopped by `start.bat` / `stop.bat`. It is written in Erlang, so Erlang/OTP 27 is
a hard prerequisite (RabbitMQ 4.3.x supports OTP 26–27; **not** 28/29 yet).

Everything is kept off `C:` via two environment variables that `start.bat` /
`stop.bat` set before launching the broker:

```bat
set "ERLANG_HOME=E:\dev\erlang"
set "RABBITMQ_BASE=E:\dev\rabbitmq\data"
set "PATH=E:\dev\erlang\bin;%PATH%"
```

> `RABBITMQ_BASE` is what relocates the node's data, logs, config and the
> `enabled_plugins` file onto `E:`. Without it, RabbitMQ defaults to
> `%APPDATA%\RabbitMQ` on `C:`.

**Connection defaults:**

| Setting        | Value                              |
| -------------- | ---------------------------------- |
| Host           | `127.0.0.1`                        |
| AMQP port      | `5672`                             |
| Management UI  | `http://127.0.0.1:15672`           |
| User / Pass    | `guest` / `guest` (localhost only) |
| Node name      | `rabbit@<HOSTNAME>`                |

> The default `guest` user can only connect over **loopback** (127.0.0.1). That's
> fine for local dev. To connect from another host, create a real user with
> `rabbitmqctl add_user` / `set_permissions`.

**Management UI:** the `rabbitmq_management` plugin is already enabled (the
`enabled_plugins` file lives at `E:\dev\rabbitmq\data\enabled_plugins`). After
`start.bat`, open `http://127.0.0.1:15672` and log in with `guest` / `guest`.

**Manual control (env must be set first — easiest from `start.bat`'s window):**

```cmd
REM start in the foreground (Ctrl+C to stop)
E:\dev\rabbitmq\sbin\rabbitmq-server.bat

REM status / stop / list queues
E:\dev\rabbitmq\sbin\rabbitmqctl.bat status
E:\dev\rabbitmq\sbin\rabbitmqctl.bat stop
E:\dev\rabbitmq\sbin\rabbitmqctl.bat list_queues
```

**PHP client – `php-amqplib` (pure PHP, no PECL extension):**

Connect to the broker from any project with Composer:

```cmd
composer require php-amqplib/php-amqplib
```

```php
use PhpAmqpLib\Connection\AMQPStreamConnection;
$conn = new AMQPStreamConnection('127.0.0.1', 5672, 'guest', 'guest');
```

> **`ext-sockets` is required** by php-amqplib. It is now enabled in both
> `php\php74\php.ini` and `php\php82\php.ini` (`extension = sockets`). The
> `php_sockets.dll` ships with both PHP builds.

> **RabbitMQ 4.x gotcha:** transient (non-durable, non-exclusive) queues are
> refused by default (`transient_nonexcl_queues` is deprecated). Declare queues
> as **durable** (`queue_declare($q, false, true, false, false)`).

**Smoke test:** a standalone publish→consume round-trip lives at
`E:\dev\tools\rabbitmq-smoketest\`. With the broker running:

```cmd
E:\dev\php\php82\php.exe E:\dev\tools\rabbitmq-smoketest\test.php
```

Expected output ends with `RESULT: OK - round trip succeeded`.

**Logs:** `E:\dev\rabbitmq\data\log\rabbit@<HOSTNAME>.log`

> ⚠️ **`epmd.exe` lingers after shutdown.** The Erlang Port Mapper Daemon stays
> resident even after `rabbitmqctl stop`; `stop.bat` force-kills `erl.exe` and
> `epmd.exe` as a fallback so the node restarts cleanly next time.

---

## 10. Adding a new project

There are two scenarios: **brand-new project** (scaffold from scratch) and  
**existing project** (copy files in, then wire up nginx).

---

### 10a. Brand-new project (green field)

```cmd
E:\dev\new-vhost.bat myapp.test php82
```

This will:

- Create `E:\dev\projects\myapp.test\public\index.php` (phpinfo stub)
- Write `E:\dev\nginx-1.31.1\conf\sites\myapp.test.conf`
- Reload Nginx automatically

Then add the hosts entry (open Notepad as Admin):

```
127.0.0.1   myapp.test
```

Replace the `public\index.php` stub with your actual application code.

---

### 10b. Existing project (copy files in)

**Step 1 â€” Place the project folder**

Copy or `git clone` your project into:

```
E:\dev\projects\<your-domain>\
```

Example: `E:\dev\projects\myapp.qhomeapps.test\`

**Step 2 â€” Choose the correct PHP version**

Check `composer.json` â†’ `"require"` â†’ `"php"`:

| composer.json requirement    | Use             |
| ---------------------------- | --------------- |
| `"php": ">=8.2"`             | `php82`         |
| `"php": ">=8.1"`             | `php82`         |
| `"php": ">=7.4"`             | `php74`         |
| No requirement / no composer | `php74` is fine |

> If you pick the wrong PHP version, you'll get a  
> `Composer detected issues in your platform` fatal error on first load.

**Step 3 â€” Identify the web root**

| Framework               | Web root               | Location                          |
| ----------------------- | ---------------------- | --------------------------------- |
| CodeIgniter 4 / Laravel | `public/` subdirectory | `E:/dev/projects/<domain>/public` |
| CodeIgniter 3           | project root           | `E:/dev/projects/<domain>`        |
| CodeIgniter 2           | project root           | `E:/dev/projects/<domain>`        |

**Step 4 â€” Generate the vhost conf**

Run `new-vhost.bat` â€” it always scaffolds with `public/` as the web root and  
uses the `_wildcard.test.pem` cert. Then edit the conf if needed:

```cmd
E:\dev\new-vhost.bat myapp.test php82
```

Open `E:\dev\nginx-1.31.1\conf\sites\myapp.test.conf` and adjust:

- **Web root is the project root (CI3/CI2):** change `root` from `.../public` to  
  the project root and add the CI internals block:

  ```nginx
  root   E:/dev/projects/myapp.test;

  location ~ ^/(application|system|vendor)/ {
      deny all;
  }
  ```
- **Third-level domain cert** (e.g. `api.mygroup.test`, see Step 5):  
  change both `ssl_certificate` lines to the correct cert group.
- **App reads `BASE_URL` from environment** (e.g. `getenv('BASE_URL')`):  
  add inside the `location ~ \.php$` block:

  ```nginx
  fastcgi_param   BASE_URL   https://myapp.test/;
  ```

**Step 5 â€” Certificate for a new domain group**

The existing certs cover: `*.test`, `*.qhomeapps.test`, `*.qhomemart.test`, `*.qhomedata.test`, `*.qhomemart.cloud.test`, `*.qhomedata.id.test`.

| Your domain                          | Cert already exists?          |
| ------------------------------------ | ----------------------------- |
| `anything.test` (one level)          | âœ… yes â€” `_wildcard.test.pem` |
| `anything.qhomeapps.test`            | âœ… yes                        |
| `anything.qhomemart.test`            | âœ… yes                        |
| `anything.qhomedata.test`            | âœ… yes                        |
| `anything.qhomemart.cloud.test`      | âœ… yes                        |
| `anything.qhomedata.id.test`         | âœ… yes                        |
| `anything.newgroup.test` (new group) | âŒ must generate              |

For a new group, run in **Admin CMD**:

```cmd
E:\dev\mkcert.exe ^
  -cert-file E:\dev\nginx-1.31.1\conf\certs\_wildcard.newgroup.test.pem ^
  -key-file  E:\dev\nginx-1.31.1\conf\certs\_wildcard.newgroup.test-key.pem ^
  "*.newgroup.test"
```

Then update the `ssl_certificate` lines in the vhost conf to point to the new cert.

**Step 6 â€” Update `base_url` in the app config**

| Framework | File                            | Setting                                        |
| --------- | ------------------------------- | ---------------------------------------------- |
| CI3       | `application/config/config.php` | `$config['base_url'] = 'https://myapp.test/';` |
| CI2       | `application/config/config.php` | `$config['base_url'] = 'https://myapp.test/';` |
| CI4       | `.env`                          | `app.baseURL = https://myapp.test/`            |
| Laravel   | `.env`                          | `APP_URL=https://myapp.test`                   |

**Step 7 â€” Add the hosts entry**

Open `C:\Windows\System32\drivers\etc\hosts` in Notepad **as Administrator**:

```
127.0.0.1   myapp.test
```

Then flush DNS:

```cmd
ipconfig /flushdns
```

**Step 8 â€” Reload Nginx**

If Nginx is already running:

```cmd
"E:\dev\nginx-1.31.1\nginx.exe" -p "E:\dev\nginx-1.31.1" -s reload
```

Or do a full restart:

```cmd
E:\dev\stop.bat
E:\dev\start.bat
```

**Step 9 â€” Verify**

Open `https://myapp.test` in the browser. No cert warning = cert is working.  
Check `E:\dev\nginx-1.31.1\logs\myapp.test.error.log` if anything fails.

---

## 11. Optional â€“ Register as a Windows service (NSSM)

> âš ï¸ **Do NOT auto-start at boot if D: is an external or removable drive.**  
> Windows may try to start the service before the drive is mounted, causing  
> repeated failures and slow boot.

1. Download NSSM from https://nssm.cc/download and extract `nssm.exe` to  
   a permanent location (e.g. `C:\tools\nssm.exe`).
1. Open **Admin CMD** and run:

```cmd
nssm install DevStack "E:\dev\start.bat"
nssm set DevStack AppDirectory "E:\dev"
nssm set DevStack Description "Local PHP dev stack (Nginx + PHP FastCGI)"
REM Start manually, not at boot:
nssm set DevStack Start SERVICE_DEMAND_START
nssm start DevStack
```

3. To remove the service later:

```cmd
nssm stop DevStack
nssm remove DevStack confirm
```

---

## 12. Verification checklist

After running `start.bat`:

- [x] `http://localhost` â†’ redirects to `https://localhost` (or 404 â€” no default vhost, that's fine)
- [x] `https://hris.qhomedata.test` â€” no certificate warning, page loads
- [x] `https://asm.qhomemart.test` â€” no certificate warning, page loads
- [x] `https://supplier.qhomemart.cloud.test` â€” no certificate warning, page loads
- [x] `https://ic-stokdigital.test` â€” no certificate warning, page loads
- [x] `https://api-qlearning.qhomeapps.test` â€” no certificate warning, page loads
- [x] `https://qlearning.qhomeapps.test` â€” no certificate warning, page loads
- [ ] `https://supplier.qhomedata.id.test` â€” no certificate warning, page loads
- [ ] PHP version per site matches the table below:

  | Site                             | Framework | PHP                              |
  | -------------------------------- | --------- | -------------------------------- |
  | `ic-stokdigital.test`            | CI3       | 7.4.33                           |
  | `supplier.qhomemart.cloud.test`  | CI2       | 7.4.33                           |
  | `asm.qhomemart.test`             | CI3       | 8.2.31 (composer requires â‰¥ 8.1) |
  | `hris.qhomedata.test`            | CI3       | 8.2.31 (composer requires â‰¥ 8.2) |
  | `api-qlearning.qhomeapps.test`   | CI4       | 8.2.31                           |
  | `qlearning.qhomeapps.test`       | CI4       | 8.2.31                           |
  | `supplier.qhomedata.id.test`     | CI2       | 7.4.33                           |
- [x] `date.timezone` shows `Asia/Jakarta` in phpinfo
- [x] `upload_max_filesize` shows `64M` in phpinfo
- [ ] RabbitMQ mgmt UI reachable at `http://127.0.0.1:15672` (guest/guest)
- [ ] `E:\dev\php\php82\php.exe E:\dev\tools\rabbitmq-smoketest\test.php` → `RESULT: OK`

**Check nginx error log** if anything fails:

```
E:\dev\nginx-1.31.1\logs\error.log
E:\dev\nginx-1.31.1\logs\<domain>.error.log
```

**Check PHP error log:**

```
E:\dev\php\php74\php_errors.log
E:\dev\php\php82\php_errors.log
```

---

## 13. Known limitations

### PHP 7.4 â€“ `zip` extension disabled

`php_zip.dll` is absent from the PHP 7.4.33 vc15 x64 NTS build. The extension
requires `zlib1.dll` (and `bz2.dll`, `zstd.dll`) which are not bundled.

To enable it later:

1. Download the dependency pack from:  
   `https://windows.php.net/downloads/php-sdk/deps/vc15/x64/`  
   Grab `zlib-*.zip` (and `bzip2-*.zip`, `zstd-*.zip` if needed).
1. Place the `.dll` files into `E:\dev\php\php74\` (the root folder, not `ext\`).
1. Download `php_zip-*-7.4-nts-vc15-x64.zip` from the PECL/windows.php.net extras.
1. Place `php_zip.dll` into `E:\dev\php\php74\ext\`.
1. Uncomment `; extension = zip` in `E:\dev\php\php74\php.ini`.
1. Restart the stack.