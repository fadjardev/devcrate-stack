# Docs and the dashboard's install picker have drifted from what the code does

**Status: open.** The documentation fixes (items 1–5, 9, 10) and the GitHub
sync (8) are done; the code fixes (6, 7, 11–13) are not. See
[Definition of done](#definition-of-done).

## Summary

A review of every built feature against [docs/roadmap.md](../roadmap.md),
[docs/cli.md](../cli.md), and [docs/tui.md](../tui.md) — done while
brainstorming what Devcrate needs to stand as a Laragon / XAMPP alternative
(see [roadmap](../roadmap.md) items 6–10) — found that several documents still
describe the stack as it was before roadmap items 2 and 4 shipped, and that the
dashboard's install picker offers options that can never succeed.

This is not a mirror of a filed GitHub issue; it is a finding from a review and
is recorded here as a write-up rather than a roadmap item. CLAUDE.md's working
agreement — *document what the code actually does* — is what each item below
breaks.

## Evidence

**Documentation that contradicts the code**

1. `docs/cli.md:1114`, under *What this deliberately does not do*: "**Never
   edits the hosts file or issues a certificate.** Both need elevation or a
   managed mkcert, which are roadmap items 4 and 2." Both are built:
   `devcrate/src/hosts.rs` writes the marked hosts block with UAC fallback and
   `devcrate/src/mkcert.rs` issues certificates; `site::create` calls
   `hosts::add_entry` (`devcrate/src/site.rs:206`) and site removal calls
   `hosts::remove_entry` (`site.rs:443`). Roadmap item 4 is marked done.
2. `docs/tui.md:139–145`, under *What it deliberately does not do*: lists
   "**Install runtimes** ... the dashboard has no screen for any of them" and
   "**Edit the hosts file or issue certificates.**" Both are false now: `i`
   opens the install picker (`Modal::InstallPicker`), and the New Site form has
   *Hosts File* and *TLS Certificate* toggles (`NewSiteForm::update_hosts`,
   `issue_tls`).
3. `docs/tui.md:60` says the install picker offers "Node.js, Bun, PHP, MariaDB,
   RabbitMQ, Composer, Nginx". `INSTALL_OPTIONS`
   (`devcrate/src/tui/app.rs:97–107`) has no nginx entry, and has a Redis entry
   the doc omits. PostgreSQL and Python, both installable from the CLI, are in
   neither.
4. `docs/index.md` — the *Contents* row for roadmap still reads "what is left —
   the runtime installer, the one-step project setup, Node and Bun", all of
   which are done, and *The stack at a glance* lists neither PostgreSQL (5432)
   nor Redis (6379).
5. `docs/roadmap.md` opens with "Today it is a set of batch scripts plus
   configuration that you drive by hand", which stopped being true at item 1.
   Its status table marks item 5 `done` while the item's own scope still lists
   unbuilt work — dev-server supervision and the HMR proxy — and item 4 marks
   `done` with certificate expiry reporting and reissue unbuilt.

**The install picker**

6. Five of the nine `INSTALL_OPTIONS` carry `None` as their version — Bun,
   MariaDB, RabbitMQ, Redis, Composer. `install_quiet` with no version lists the
   catalogue and installs nothing, so `Job::Install`
   (`devcrate/src/tui/worker.rs:381–392`) reports each as a failure: "No version
   named for <runtime> -- nothing installed." Choosing them always fails. This
   was noted as out of scope in
   [tui-install-stdout-corruption-2026-09-19.md](tui-install-stdout-corruption-2026-09-19.md)
   (then four of eight) and is still open; Redis has since joined the list.
7. The versions that *are* named are baked into the binary — `Node.js
   (v22.11.0)`, `PHP 8.4` / `8.3` / `8.2` — rather than read from the same
   upstream catalogues the CLI uses, which roadmap item 2 requires ("resolved
   from the upstream sources rather than a list baked into the binary"). They
   will go stale without a rebuild.

**The GitHub issues and their mirrors**

8. The issues on GitHub have not been ticked since they were filed, while the
   mirrors here have. Checked with `gh issue view` on 2026-10-15:

   | Issue | GitHub: ticked / open | Mirror: ticked / open | GitHub state |
   | --- | --- | --- | --- |
   | #1 Rust TUI | 0 / 16 | 19 / 0 | open |
   | #2 Runtime installer | 0 / 11 | 15 / 3 | open |
   | #3 Run from any terminal | 10 / 5 | 18 / 1 | open |

   None of the three has a comment recording progress. #1 is complete by its own
   mirror and is still open.
9. The mirror itself is behind on #2: `docs/issue/issue-2.md:112–113` still
   reads "Remove a version — *not built; still `rmdir`.*" and leaves the
   vhost-still-points-at-it guard unticked. `devcrate uninstall` built both
   (`devcrate/src/uninstall.rs`; roadmap item 2, *Built for uninstall too*).
10. The mirrors still say "Roadmap item N of 3" though the roadmap has had five
    done items, and items 4 and 5 have no GitHub issue at all.

**Found while fixing the docs**

11. The dashboard's `d` does not remove the host from the hosts file. The
    CLI's `site::remove` (`devcrate/src/site.rs:438–449`) calls
    `hosts::remove_entry` in its *printing* wrapper, after `site::delete`;
    `Job::SiteRemove` (`devcrate/src/tui/worker.rs:350`) calls `site::delete`
    alone. The work belongs in the shared core — the rule CLAUDE.md calls *one
    core, two front ends*. Running `devcrate site remove` afterwards cannot
    clean up either: the conf is already gone, so it refuses before reaching
    the hosts file.
12. The unelevated hosts write is not atomic. `hosts::write_hosts_content`
    (`devcrate/src/hosts.rs:148`) calls `fs::write` on the hosts file itself;
    only the elevated path goes through a temp file. Roadmap item 4's
    constraint is "write to a temp file, then replace". An interrupted write
    can leave the system-wide hosts file truncated (`hosts.bak` holds the
    copy from before the first edit only).
13. Removing the PHP version `php\current` names does not need `--force`.
    `uninstall_php` (`devcrate/src/uninstall.rs:150–200`) records
    `was_current` and clears the junction, but never refuses, while nginx,
    Node, Bun, and Python all do — and the module doc, `cli.md`, and the
    changelog all say PHP does too.

## Impact

Items 1–5 make the docs undersell the tool: a reader of `cli.md` or `tui.md`
would conclude hosts editing, TLS issuance, and dashboard installs do not
exist. Items 6–7 are user-visible defects: more than half the install picker
is a dead end, and the remainder will drift from what the vendors ship.

## Proposed fix

1. Rewrite the two *does not do* sections (`cli.md`, `tui.md`) to what is
   actually still not done: editing a vhost beyond its PHP version, certificate
   expiry reporting, and — for the dashboard — live install progress.
2. Fix the picker list in `tui.md:60`, the roadmap row in `index.md`, add
   PostgreSQL and Redis to *The stack at a glance*, and replace the opening
   paragraph of `roadmap.md`. Split the open scope of items 4 and 5 out of
   their `done` rows (or mark the rows `done, with gaps` and name them).
3. Give every `None` picker option a real default to resolve — the newest
   stable line from the runtime's catalogue, the same one `devcrate install
   <runtime> <line>` would take — so "Latest" installs something.
4. Build the picker's entries from the catalogue functions at open time
   (cached, with the current hard-coded list as an offline fallback), and add
   nginx, PostgreSQL, and Python.
5. Tick `issue-2.md`'s uninstall lines to match `uninstall.rs`. Then bring the
   GitHub issues level with their mirrors — edit each body to the mirror's
   checkboxes, and close #1 (and #2 if the catalogue cache is moved to its own
   issue). Filing issues for items 4–10 is the owner's call, per CLAUDE.md.
6. Move the hosts removal into `site::delete` (or a shared core function
   both front ends call), so `d` and `site remove` do the same thing (11).
7. Write the hosts file to a temp file beside it and rename over it when no
   elevation is needed (12).
8. Add the `was_current && !force` refusal to `uninstall_php`, matching the
   other versioned runtimes, with a test (13).

Fixes 1–2 are documentation only. Fixes 3–4 touch `tui/app.rs`, `tui/worker.rs`
and the catalogue code in `install/`; 6–8 touch `site.rs`, `hosts.rs`, and
`uninstall.rs`.

## Out of scope

- Live download progress inside the dashboard — a separate follow-up, already
  named in the 2026-09-19 write-up.
- Wiring `devcrate uninstall` into the dashboard.

## Definition of done

- [x] `cli.md` and `tui.md` *does not do* sections describe only what is
      genuinely not built.
- [x] `tui.md`'s picker list matches `INSTALL_OPTIONS` exactly.
- [x] `index.md` lists every managed service and an accurate roadmap summary.
- [x] `roadmap.md`'s opening and status table no longer hide open scope.
- [ ] Every option in the install picker installs something, or is removed.
- [ ] The picker's versions come from the upstream catalogues, with an offline
      fallback; nginx, PostgreSQL, and Python are offered.
- [x] `issue-2.md` reflects that uninstall is built.
- [ ] The dashboard's `d` removes the hosts entry, as `site remove` does.
- [ ] An unelevated hosts write is atomic (temp file, then rename).
- [ ] `uninstall php` refuses the `current` version without `--force`.
- [x] Each GitHub issue's checkboxes match its mirror; finished issues are
      closed. — *Bodies of #1–#3 replaced with their mirrors on 2026-10-15,
      after the mirrors' own stale notes were corrected (#1's site creation,
      #2's `my.ini` and definition of done, #3's hosts entry). #1 closed as
      completed; #2 (catalogue cache) and #3 (platform abstraction) stay open.*
- [ ] `cargo build --release` is warning-free and `cargo test` passes.
