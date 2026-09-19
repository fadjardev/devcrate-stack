# TUI install jobs write raw stdout while the alternate screen is active

**Status: fixed.** See [Resolution](#resolution) below.

## Summary

`devcrate architecture.md` states the core design rule: *"The dashboard must
never call anything that writes to stdout — stdout is the screen it just
drew."* `Job::Install` in `devcrate/src/tui/worker.rs` breaks that rule: it
calls `install::install()` directly from the background worker thread, and
that function prints unconditionally — including a raw `\r`-carriage-return
progress bar — straight to the same stdout handle ratatui is redrawing from
the main thread. Starting an install from the dashboard can corrupt or
interleave with the TUI's alternate-screen output.

This is not a mirror of a filed GitHub issue; it is a finding from a code
audit and is recorded here as a bug write-up rather than a roadmap item.

## Evidence

- `devcrate/src/tui/worker.rs:365` — `Job::Install` calls
  `crate::install::install(stack, &runtime, version.as_deref(), None, true)`
  on the worker thread, the exact same function the CLI uses for
  `devcrate install <runtime>`.
- `devcrate/src/install/mod.rs` prints unconditionally throughout:
  - `choose_php`, `choose_nginx`, `choose_composer`, `choose_postgres`,
    `choose_python`, `choose_mariadb`, `choose_rabbitmq`, `choose_erlang`,
    `choose_node`, `choose_bun` all call `println!` directly (e.g. lines
    1555, 1590, 1607, 1639, 1687, 1734…).
  - `print_installed` / `print_<runtime>_installed` (e.g. lines 2137, 2476,
    2519…) print a summary table after every install.
  - `Reporter::redraw` (line 2100) does `print!("\r{}", self.last)` followed
    by an explicit `stdout().flush()`, gated only on
    `stdout().is_terminal()` (line 2054) — which is still `true` while
    ratatui holds the alternate screen, since it is the same stdout handle.
- No other `Job` variant does this. `SiteAdd`, `SiteSetPhp`, and
  `SiteRemove` all call a pure core function (`site::create`,
  `site::set_php`, `site::delete`) that returns a plain struct with no I/O,
  and `worker.rs` builds the `JobResult` text itself. `install::install` has
  no such pure counterpart — the CLI's printing and the installer's actual
  work are fused into one function.
- `install/mod.rs` is 2,809 lines, the largest file in the project by a wide
  margin (next largest, `install/download.rs`, is 1,357). The fused
  print/logic path lives inside it, which likely made this divergence from
  the rest of the codebase's pure-core/thin-print-wrapper pattern easy to
  miss in review.
- Zero test coverage touches this path: `tui/worker.rs` has no `#[test]`
  functions, and unit-testing a background-thread stdout mutation against a
  live alternate screen isn't practical with the existing `TestBackend`
  rendering tests (see `devcrate/src/tui/mod.rs`).

## Impact

Any install started from the dashboard (`devcrate` with no arguments, using
the install action wired to `Job::Install`) writes raw, uncoordinated text to
the terminal while the render thread is concurrently redrawing the
alternate-screen buffer. At minimum this produces visual corruption during
the install; depending on terminal and timing it can also leave stray output
behind after the TUI exits.

## Proposed fix

Split `install::install()` the same way `site.rs` already splits
`create`/`add`:

1. Add a pure core (e.g. keep `from_archive` / `install_by_download` as the
   logic, but stop them constructing a `Reporter` or calling `print_installed`
   internally). Progress should flow out through the callback parameter that
   already exists (`&mut |progress| ...`), not through `println!`/`print!`.
2. Give the CLI its own thin wrapper — construct the terminal `Reporter`
   there, call the pure core, then call `print_installed`. This is the
   existing pattern in `site.rs` (`create` vs `add`) and `uninstall.rs`.
3. Have `tui/worker.rs`'s `Job::Install` arm call the pure core with a
   callback that forwards progress into its existing `Sender<Msg>` channel
   instead of constructing a `Reporter`, and build `JobResult` from the
   returned `Installed` value — matching `SiteAdd`/`SiteSetPhp`/`SiteRemove`.

Doing this alongside a light pass to move each runtime's `choose_*`/`print_*`
pair out of `install/mod.rs` and into its existing per-runtime file
(`install/php.rs`, `install/nginx.rs`, etc.) would also address the file's
size and make `mod.rs` pure orchestration, matching the split every other
runtime already has partially in place.

## Out of scope

- Rewriting `install/download.rs` or the per-vendor verification logic —
  unrelated to this bug and not oversized in a way that hides defects.
- Wiring `devcrate uninstall` into the TUI — it currently has no `Job`
  variant at all, so it cannot hit this class of bug; that is a separate
  roadmap gap, not a regression.

## Resolution

Fixed as proposed above, with one refinement and one adjacent bug closed
along the way:

- Every `println!`/`print!` in `install/mod.rs` (choose\_\*, the per-runtime
  catalogues and guidance, the `print_*_installed` family, `report_transfer`,
  and the download/extract progress bar) now writes through an injected
  `Emit` trait object instead of stdout directly, via a `emit_line!` macro.
  `TerminalEmit` reproduces the CLI's exact previous output (verified
  byte-for-byte against `devcrate install php` and `devcrate install
  mariadb`); `NullEmit` discards everything.
- `install()` is now a thin wrapper: it builds a `TerminalEmit` and calls
  `install_with`, the pure core. `install_quiet()` is the dashboard's entry
  point — same core, `NullEmit`, returning an `InstallOutcome` instead of
  printing a summary.
- `tui/worker.rs`'s `Job::Install` now calls `install_quiet` and builds its
  `JobResult` from the returned `InstallOutcome`, the same shape
  `SiteAdd`/`SiteSetPhp`/`SiteRemove` already use. It does not forward live
  download/extract progress into the dashboard — `NullEmit` discards it, same
  as every other job type today, which shows only a final result, not
  progress while it runs. Surfacing live install progress in the dashboard is
  a follow-up, not part of this fix.
- Found while fixing this: `install`'s "no version named" branch (used by 4
  of the TUI's 8 `INSTALL_OPTIONS` — Bun, MariaDB, RabbitMQ, Composer
  "Latest") lists the catalogue and installs nothing, but the dashboard
  reported "Successfully installed" regardless. Now reported as a failed
  `JobResult` naming the CLI command to run instead. Choosing real default
  versions for those four options (so "Latest" actually installs something)
  is a separate, larger change and is out of scope here.

## Definition of done

- [x] `install::install` (or an equivalent pure function) performs an
      install with no direct `println!`/`print!` calls, taking a progress
      callback instead.
- [x] The CLI's `devcrate install <runtime>` keeps identical terminal output
      (progress bar + summary) via a thin wrapper around the pure core.
- [x] `tui/worker.rs`'s `Job::Install` uses the pure core and reports its
      result through the existing `JobResult` channel, with no writes to
      stdout from the worker thread.
- [x] `cargo build --release` is warning-free and `cargo test` passes (106
      tests). A full manual TUI check per `docs/tui.md` (actually starting an
      install from the dashboard and watching the alternate screen) has not
      been run and is recommended before this is considered fully verified.
