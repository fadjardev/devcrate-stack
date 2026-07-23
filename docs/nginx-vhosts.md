# Nginx and virtual hosts

Nginx serves every project over HTTPS on `:443` (with `:80` redirecting to
HTTPS). The main config is `nginx\conf\nginx.conf`, which includes every
per-project vhost from `conf\sites\*.conf`.

## Current sites

| Host | Framework | PHP | Web root (under `projects\`) |
| --- | --- | --- | --- |
| `api-qlearning.qhomeapps.test` | CodeIgniter 4 | 8.2 | `api-qlearning.qhomeapps.id\public` |
| `qlearning.qhomeapps.test` | CodeIgniter 4 | 8.2 | `qlearning.qhomeapps.id\public` |
| `asm.qhomemart.test` | CodeIgniter 3 | 8.2 | `asm.qhomemart.cloud` |
| `hris.qhomedata.test` | CodeIgniter 3 | 8.2 | `hris.qhomedata.id` |
| `ic-stokdigital.test` | CodeIgniter 3 | 7.4 | `ic-stokdigital` |
| `supplier.qhomedata.id.test` | CodeIgniter 2 | 7.4 | `supplier.qhomedata.id` |
| `supplier.qhomemart.cloud.test` | CodeIgniter 2 | 7.4 | `supplier.qhomemart.cloud` |

## Anatomy of a vhost

Each `conf\sites\<host>.conf` has an HTTP->HTTPS redirect server and an HTTPS
server:

```nginx
server {
    listen       80;
    server_name  myapp.test;
    return 301   https://$host$request_uri;
}

server {
    listen       443 ssl;
    server_name  myapp.test;

    root   projects/myapp.test/public;   # web root (prefix-relative, see note)
    index  index.php index.html;

    ssl_certificate      certs/_wildcard.test.pem;       # conf-relative
    ssl_certificate_key  certs/_wildcard.test-key.pem;

    location / {
        try_files $uri $uri/ /index.php?$query_string;
    }

    location ~ \.php$ {
        try_files       $uri =404;
        fastcgi_pass    127.0.0.1:9082;          # <- selects PHP version
        fastcgi_index   index.php;
        include         fastcgi_params;
        fastcgi_param   SCRIPT_FILENAME  $document_root$fastcgi_script_name;
        fastcgi_param   HTTPS            on;
    }
}
```

The `fastcgi_pass` port is what pins the site to a PHP version:
`9074` = 7.4, `9082` = 8.2, `9085` = 8.5.

**Why the paths are relative.** Nothing in a vhost conf names the stack root, so
the whole folder can move without touching any config:

- `root projects/<domain>/...` and `access_log logs/...` resolve against the
  nginx **prefix** (`<stack-root>\nginx`, set by `-p` in `start.bat`).
  The `projects` path goes through a junction
  `nginx\projects -> <stack-root>\projects` that `start.bat` and
  `new-vhost.bat` create automatically. The junction keeps `SCRIPT_FILENAME`
  free of `..` — PHP-CGI on Windows refuses paths containing `..`
  ("No input file specified"), so don't replace it with `root ../projects/...`.
- `ssl_certificate certs/...` resolves against the **conf** directory
  (`<prefix>\conf`), not the prefix — hence no `conf/` in front.

## Adding a project vhost

Use the helper (from anywhere, since the stack root is on PATH):

```bat
new-vhost.bat myapp.test 8.5
```

It scaffolds `projects\myapp.test\public\index.php` (a phpinfo stub), writes
`conf\sites\myapp.test.conf` with the `public/` web root and the `*.test`
wildcard cert, and reloads nginx. The PHP argument is resolved against the
`php\php-*` folders that are actually installed, and may be spelled `8.5`, `85`,
or `php-8.5`.

`devcrate site add myapp.test --php 8.5` does the same and tests the
configuration before reloading; `devcrate site set-php myapp.test 7.4` changes
an existing vhost's version without touching the rest of its conf. See
[cli.md](cli.md).

Then add the printed hosts line (Notepad as Admin):

```
127.0.0.1   myapp.test
```

### Adjustments after scaffolding

`new-vhost.bat` always assumes a `public/` web root and the `*.test` cert. Edit
the generated conf when the project differs:

- **Web root is the project root (CodeIgniter 2/3):** change `root` to the
  project folder and block framework internals:

  ```nginx
  root   projects/myapp.test;

  location ~ ^/(application|system|vendor)/ {
      deny all;
  }
  ```

- **Third-level domain** (e.g. `api.mygroup.test`): point both
  `ssl_certificate` lines at the matching `*.mygroup.test` cert (see below).

- **App reads `BASE_URL` from the environment:** add inside the
  `location ~ \.php$` block:

  ```nginx
  fastcgi_param   BASE_URL   https://myapp.test/;
  ```

### Per-framework web root

| Framework | Web root | `root` should point at |
| --- | --- | --- |
| CodeIgniter 4 / Laravel | `public/` subdir | `projects\<domain>\public` |
| CodeIgniter 3 | project root | `projects\<domain>` |
| CodeIgniter 2 | project root | `projects\<domain>` |

### Per-framework base URL

| Framework | File | Setting |
| --- | --- | --- |
| CI2 / CI3 | `application/config/config.php` | `$config['base_url'] = 'https://myapp.test/';` |
| CI4 | `.env` | `app.baseURL = https://myapp.test/` |
| Laravel | `.env` | `APP_URL=https://myapp.test` |

## TLS certificates with mkcert

Browsers trust these certs because the mkcert local CA is installed into the
Windows certificate store (`mkcert -install`, run once as Admin - see
[installation.md](installation.md#6-mkcert--tls-certificates-admin)).

### Wildcard strategy

X.509 wildcards are single-level: `*.test` matches `ic-stokdigital.test` but NOT
`api.qhomeapps.test` (that is a third level). Multi-level wildcards like
`*.*.test` are invalid. So each domain group gets its own wildcard cert.

Existing certs cover:

- `*.test`
- `*.qhomeapps.test`
- `*.qhomemart.test`
- `*.qhomedata.test`
- `*.qhomemart.cloud.test`
- `*.qhomedata.id.test`

### Generating certs (Admin CMD)

One command per group, writing into `nginx\conf\certs\`:

```cmd
C:\devcrate\mkcert.exe ^
  -cert-file C:\devcrate\nginx\conf\certs\_wildcard.test.pem ^
  -key-file  C:\devcrate\nginx\conf\certs\_wildcard.test-key.pem ^
  "*.test"

C:\devcrate\mkcert.exe ^
  -cert-file C:\devcrate\nginx\conf\certs\_wildcard.qhomeapps.test.pem ^
  -key-file  C:\devcrate\nginx\conf\certs\_wildcard.qhomeapps.test-key.pem ^
  "*.qhomeapps.test"

C:\devcrate\mkcert.exe ^
  -cert-file C:\devcrate\nginx\conf\certs\_wildcard.qhomemart.test.pem ^
  -key-file  C:\devcrate\nginx\conf\certs\_wildcard.qhomemart.test-key.pem ^
  "*.qhomemart.test"

C:\devcrate\mkcert.exe ^
  -cert-file C:\devcrate\nginx\conf\certs\_wildcard.qhomedata.test.pem ^
  -key-file  C:\devcrate\nginx\conf\certs\_wildcard.qhomedata.test-key.pem ^
  "*.qhomedata.test"

C:\devcrate\mkcert.exe ^
  -cert-file C:\devcrate\nginx\conf\certs\_wildcard.qhomemart.cloud.test.pem ^
  -key-file  C:\devcrate\nginx\conf\certs\_wildcard.qhomemart.cloud.test-key.pem ^
  "*.qhomemart.cloud.test"

C:\devcrate\mkcert.exe ^
  -cert-file C:\devcrate\nginx\conf\certs\_wildcard.qhomedata.id.test.pem ^
  -key-file  C:\devcrate\nginx\conf\certs\_wildcard.qhomedata.id.test-key.pem ^
  "*.qhomedata.id.test"
```

For a brand-new group, generate `*.newgroup.test` the same way and update the
`ssl_certificate` lines in that group's vhost conf.

> These `.pem` files (certs and keys) are excluded from the repo. Regenerate
> them after cloning; they are machine-specific and the private keys must never
> be committed.

## Reloading nginx

After editing a conf:

```cmd
"C:\devcrate\nginx\current\nginx.exe" -p "C:\devcrate\nginx" -s reload
```

Or a full restart with `stop.bat` then `start.bat`. Check
`nginx\logs\<domain>.error.log` if a site fails to load.
