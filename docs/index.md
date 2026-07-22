# Documentation

Comprehensive reference for **Devcrate**, the portable multi-PHP development stack
at `E:\dev`.

For a quick overview and the fastest path to a running stack, start with the
root [README.md](../README.md), then [setup.md](setup.md) for the linear
install walkthrough.

## Contents

| Doc | What it covers |
| --- | --- |
| [setup.md](setup.md) | Linear walkthrough to get the stack running from a fresh clone. |
| [architecture.md](architecture.md) | How the pieces fit together: folder layout, request flow, ports, and the design decisions behind the stack. |
| [installation.md](installation.md) | Rebuild the whole stack from a fresh clone: what to download, where it goes, and how to wire it up. |
| [php-versions.md](php-versions.md) | Running multiple PHP versions: the `phpuse` CLI switcher, per-version FastCGI, `php.ini`, and adding a version. |
| [nginx-vhosts.md](nginx-vhosts.md) | Nginx config, adding a project vhost, TLS certificates with mkcert, and per-framework web roots. |
| [database.md](database.md) | MariaDB usage: connecting, creating databases, imports, and per-framework DB config. |
| [rabbitmq.md](rabbitmq.md) | RabbitMQ + Erlang: connecting, the management UI, the PHP client, and the smoke test. |
| [troubleshooting.md](troubleshooting.md) | Common failures, known limitations, the verification checklist, and log locations. |

## The stack at a glance

| Service | Version | Endpoint | Credentials |
| --- | --- | --- | --- |
| Nginx | 1.31.1 | `:80` / `:443` | - |
| PHP (CLI) | 7.4 / 8.2 / 8.5 | switch with `phpuse` | - |
| PHP (FastCGI) | per vhost | `:9074` / `:9082` / `:9085` | - |
| MariaDB | 12.3 | `127.0.0.1:3306` | `root`, no password |
| RabbitMQ | 4.3.2 (Erlang 27) | `:5672`, UI `:15672` | `guest` / `guest` |

Everything runs out of `E:\dev` with no system-wide installs. Only the scripts
and service configs are versioned; binaries, runtime data, TLS keys, and
`projects/` are excluded (see [.gitignore](../.gitignore)).
