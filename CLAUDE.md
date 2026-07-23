# Devcrate — notes for Claude Code

A portable, multi-PHP development stack for Windows. Nginx 1.31.1 + PHP 7.4 /
8.2 / 8.5 as `php-cgi.exe` FastCGI workers (ports 9074 / 9082 / 9085) + MariaDB
12.3 + RabbitMQ 4.3.2 (Erlang/OTP 27), serving per-project `.test` vhosts over
HTTPS with mkcert wildcard certificates.

Two ways to drive it, both kept working: the batch scripts in the stack root
(`start.bat`, `stop.bat`, `phpuse.bat`, `new-vhost.bat`) and the Rust binary
under `devcrate/` — a dashboard (`devcrate` with no arguments) plus scriptable
subcommands.

**This repo holds configuration only.** Runtime binaries, archives, TLS keys,
and the application code under `projects/` are deliberately not committed.

## Where to read

| For | Read |
| --- | --- |
| What's built and what's planned, in order | [docs/roadmap.md](docs/roadmap.md) — the status source of truth |
| The subcommands | [docs/cli.md](docs/cli.md) |
| The dashboard | [docs/tui.md](docs/tui.md) |
| Layout, request flow, design decisions | [docs/architecture.md](docs/architecture.md) |
| Known failures and their causes | [docs/troubleshooting.md](docs/troubleshooting.md) |
| The GitHub issues | [docs/issue/](docs/issue) — mirrors, kept ticked to match reality |

## Build and test

```bat
cd devcrate
cargo build --release
cargo test
```

Keep it at zero warnings. `E:\dev\devcrate.exe` is a **copy** of the release
build (gitignored) so `devcrate` resolves on `PATH` — re-copy it after every
build or you are running the old binary.

The stack is usually running; `devcrate status` says. Stop it before renaming or
replacing anything under `php\` or `nginx\`.

## Constraints that must not regress

Each of these cost real effort to find.

- **Location independence.** The stack runs from *any* folder — "the stack
  root" — not just `E:\dev`. Every `.bat` resolves its own root from `%~dp0`;
  the binary resolves it from `--root`, then `DEVCRATE_HOME`, then its own
  location, then the working directory. No hard-coded `E:\dev` anywhere. Docs
  use `C:\devcrate` as a stand-in example root.
- **nginx has two path bases.** `root`, `access_log`, and `error_log` resolve
  against the *prefix*; `ssl_certificate` and `ssl_certificate_key` against the
  *conf directory*. Generated vhosts therefore say `root projects/<host>/public`,
  `certs/_wildcard.test.pem`, `logs/<host>.access.log`.
- **The nginx prefix is `nginx\` and does not move.** It holds what belongs to
  the *stack* — `conf\` (vhosts + certs), `logs\`, `temp\`, the `projects`
  junction — while each build lives in `nginx\nginx-<version>\` and
  `nginx\current` names the active one. Both path bases above therefore sit
  outside the versioned folder, which is exactly why no vhost conf mentions a
  version or needs rewriting to switch. The mirror image of PHP, and for the
  opposite reason: PHP versions coexist and each is self-contained, whereas one
  nginx runs and its config belongs to the stack. A directory is recognised as
  a prefix by holding `conf\nginx.conf`, not by its name, so the pre-restructure
  layout (`nginx-1.31.1\nginx.exe` beside its own `conf\`) still runs;
  `devcrate nginx migrate` converts it. Don't reintroduce a hard-coded
  `nginx-1.31.1`.
- **PHP-CGI on Windows rejects any `SCRIPT_FILENAME` containing `..`** — it
  answers "No input file specified", and `$realpath_root` is a no-op on win32.
  That is why `nginx\projects` is a directory junction to `..\projects`:
  it keeps document roots dot-free. `mklink /J` needs no Administrator, and the
  same trick powers `php\current` and `nginx\current` (all three go through
  `junction.rs`).
- **PHP versions are folders**, named `php-<X.Y>` (`php-7.4`, `php-8.2`,
  `php-8.5`). Both the display version and the FastCGI port (`90` + the digits)
  are derived from the folder name, so unpacking `php-8.4\` is the whole of
  installing a version. Everything matches on the digits alone, which makes
  `8.5`, `85`, `php-8.5`, and the older `php85` interchangeable everywhere a
  version is named.
- **One core, two front ends.** Each action is a function returning a structured
  result (`control::run_start`, `site::create`, `php::use_version`) with the
  printing in a thin CLI wrapper. The dashboard must never call anything that
  writes to stdout — stdout is the screen it just drew.
- **Never commit** binaries, downloaded archives, runtime data, `projects/`, or
  TLS private keys (`nginx/conf/certs/*-key.pem`, plus a repo-wide `*-key.pem`
  rule). Check `.gitignore` before adding anything.

## Working agreements

- **Commit and push only when asked.** Offering a commit message is welcome;
  running `git commit` uninvited is not.
- **Don't create GitHub labels, milestones, or issues** — the owner does that.
- `CHANGELOG.md` follows Keep a Changelog + SemVer. Log user-visible changes
  under `[Unreleased]` with `### Added` / `### Changed` / `### Fixed`. Leave
  released sections alone: they describe what shipped then, not what is true now.
- **Document what the code actually does, not what it ought to do.** Read the
  `.bat` or the `.rs` before writing about it.
- Prefer an honest "not delivered, and here is why" over quietly dropping scope.
  The roadmap and the issue mirrors carry several of these; they are load-bearing.

## Environment

- **`gh` is not installed**, and non-interactive git cannot authenticate to the
  remote — `git ls-remote` fails with "Repository not found" even though the
  repo exists and pushes work. Don't chase it. The repo is private, so anonymous
  web fetches of it 404 too; that is why the issues are mirrored into
  `docs/issue/`.
- **The dashboard cannot be driven from tool calls.** It needs a real terminal
  and refuses to start without one. Verify it by rendering into ratatui's
  `TestBackend` and reading the buffer back as text — see the tests in
  `devcrate/src/tui/mod.rs`.
- Windows: PowerShell and Git Bash are both available and take different syntax.
  A standalone Git Bash (mintty) window hands programs an MSYS pipe rather than
  a console, so the dashboard's key handling needs Windows Terminal, PowerShell,
  cmd, or the VS Code integrated terminal.
