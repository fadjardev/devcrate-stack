# Run from any terminal: one binary on PATH with scriptable subcommands #3

## Summary

Devcrate should not be tied to one shell or to double-clicking a `.bat` file. Ship one
`devcrate` binary that behaves identically in `cmd.exe`, PowerShell, Windows Terminal,
Git Bash, and an IDE's integrated terminal — with non-interactive subcommands
alongside the TUI, from the same executable.

Roadmap item 3 of 3 — see [docs/roadmap.md](docs/roadmap.md).

## Motivation

The batch scripts assume `cmd.exe` and a hard-coded `E:\dev`. That means the stack
cannot be driven from a task runner, a keybinding, an IDE, or CI, and it cannot live
anywhere other than `E:\dev`. The subcommand surface is also what makes the TUI (#1)
implementable as a thin front end rather than a second implementation.

## Scope

### Command surface
- [ ] `devcrate` — launch the TUI (today: prints help and exits 2; the TUI is #1)
- [x] `devcrate start [service]` / `devcrate stop [service]` / `devcrate restart`
- [x] `devcrate status` (with `--json` for machine-readable output)
- [x] `devcrate php use 8.5` — accepts `8.5`, `85`, `php-8.5`, or the older `php85`
- [ ] `devcrate install php 8.5` (surface reserved for #2) — declared, exits 3
- [x] `devcrate site add myapp.test --php 8.5` — plus `site set-php` and `site remove`
- [x] `devcrate --version`, `--help`, and per-subcommand help

### Path and config handling
- [x] Resolve the stack root from, in order: an explicit flag, the `DEVCRATE_HOME`
      environment variable, the executable's own location, then the working
      directory — the last two searching upward. A `devcrate.toml` *marks* a
      directory as a stack root rather than supplying the path, since the file
      has to be found before it can be read.
- [x] `devcrate.toml` holds installed runtime versions and port assignments. Not
      the stack root: locating the file is what establishes it.
- [x] No hard-coded `E:\dev` anywhere in the codebase

### Shell and terminal behaviour
- [x] Detect ANSI support and degrade cleanly where it is missing. `NO_COLOR`
      wins outright, `CLICOLOR_FORCE` forces it through a pipe, `TERM=dumb` and
      a non-terminal stdout disable it, and a Windows console gets
      `ENABLE_VIRTUAL_TERMINAL_PROCESSING` set before anything is emitted.
      Truecolor is not detected because nothing here uses it — the palette is
      four basic SGR codes, which every one of the target terminals renders.
- [x] Never assume a fixed terminal width. It is read from `COLUMNS` first, then
      the console screen buffer; when neither answers, nothing is wrapped or
      truncated rather than falling back to 80.
- [ ] Restore the terminal (alternate screen, cooked mode) on exit *and* on panic
      — nothing to restore until the TUI exists. Neither mode is entered today.
- [x] Correct exit codes, so `devcrate start && …` behaves in a script
      (0 ok, 1 error, 2 usage, 3 not implemented)
- [x] Standard handles are not leaked to spawned services, so `devcrate start`
      redirected to a file or a pipe returns instead of hanging until the stack
      stops — which `start.bat` does not do

### Portability groundwork
- [ ] Keep platform-specific code (junction vs symlink, archive formats, process
      signalling) behind a thin abstraction, so a Linux/macOS build is a follow-up
      rather than a rewrite

## Technical notes

Nothing in the stack is Windows-specific except the runtime archives and the
`php\current` junction. Getting the path abstraction right here is what makes a
future non-Windows build cheap — this issue is where that call gets made.

Suggested crates: `clap` (derive), `serde` + `toml`, `anyhow`/`thiserror`.

## Out of scope

- Actually implementing a Linux/macOS build
- The TUI itself (#1) and the runtime installer (#2) — this issue defines the
  plumbing both sit on

## Dependencies

None. Per the roadmap's build order this lands **first**: project skeleton, config
model, stack-root resolution, and the CLI surface — then the existing scripts get
ported behind those subcommands one at a time.

## Migration

The batch scripts stay in the repo and keep working until the equivalent subcommand
ships and is documented. Retire them one at a time, not in a big bang.

All four now have an equivalent — `start.bat` → `devcrate start`, `stop.bat` →
`devcrate stop`, `phpuse.bat` → `devcrate php use`, `new-vhost.bat` →
`devcrate site add`. Nothing has been retired yet, and nothing should be until
the binary has been driven day to day for a while: the scripts are the fallback
if the binary is missing or a build breaks.

## Definition of done

- [x] `devcrate status` returns correct output from cmd, PowerShell, and Git Bash
- [x] The stack runs from a directory other than `E:\dev` with no source changes
- [x] `start`, `stop`, `status`, `php use`, and `site add` are at parity with the
      corresponding batch scripts — and ahead of them on process scoping, port
      preflight, naming the process that holds a port, config testing, and
      overwrite protection. The hosts-file entry stays manual in both (see #4).