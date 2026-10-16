# Releasing

How a version of Devcrate is cut, tagged, and published. This is the procedure
v0.2.0 was released with, plus the release zip added after it; follow it in
order. Everything up to [Publish](#6-publish) is local and can be undone;
everything from there on is public, so it is done only with the owner's
go-ahead.

## Versioning

- **Semantic Versioning**, `MAJOR.MINOR.PATCH`, with the changelog in
  [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) form — see
  [CHANGELOG.md](../CHANGELOG.md).
- **Still 0.x.** Before 1.0, a release that adds features bumps MINOR
  (0.1.0 → 0.2.0); one that only fixes bugs bumps PATCH (0.2.0 → 0.2.1). A
  change that breaks how an existing stack runs — a layout it no longer
  recognises, a subcommand that changes meaning — is still a MINOR bump while
  on 0.x, and the changelog says so under **Changed**.
- **One version, three places**, kept equal: the changelog heading
  `## [X.Y.Z]`, `version` in `devcrate/Cargo.toml` (which `devcrate --version`
  reports), and the git tag `vX.Y.Z`.

## Branches and tags

- Work lands on **`dev`**. **`main`** is the released line and only ever moves
  by fast-forward to a release commit on `dev` — never a merge commit, never a
  commit of its own. If `main` is not an ancestor of `dev`, stop and sort that
  out first; do not force-push either branch.
- Tags are **annotated**, named `vX.Y.Z`, on the release commit, with the
  message `Devcrate vX.Y.Z - <a few words on what it is>`:

  ```
  v0.1.0   Devcrate v0.1.0 - first prototype
  v0.2.0   Devcrate v0.2.0 - dashboard, runtime installer, one-step sites
  ```

- A pushed tag is never moved or deleted. A mistake in a release is fixed by
  the next patch release.

## 1. Check the starting point

```sh
git fetch origin
git status                                   # nothing uncommitted that belongs in the release
git rev-list --count main..dev               # what the release contains
git merge-base --is-ancestor main dev && echo "fast-forward possible"
git tag -l                                   # the previous tag exists...
gh api repos/fadjardev/devcrate-stack/tags   # ...and is on GitHub, not only local
```

v0.1.0 was tagged locally and never pushed, which left the changelog's links
pointing at nothing for three months; the last line is what catches that.

## 2. Cut the changelog

In [CHANGELOG.md](../CHANGELOG.md):

1. Rename `## [Unreleased]` to `## [X.Y.Z] - YYYY-MM-DD` (today's date), and
   add a fresh, empty `## [Unreleased]` above it.
2. Make sure the new section has **one** `### Added`, **one** `### Changed`,
   and **one** `### Fixed`, in that order. Entries logged over months tend to
   open a second heading of the same kind further down; merge them by moving
   the entries, never by rewording them.
3. Update the link references at the bottom:

   ```
   [Unreleased]: https://github.com/fadjardev/devcrate-stack/compare/vX.Y.Z...HEAD
   [X.Y.Z]: https://github.com/fadjardev/devcrate-stack/compare/vPREV...vX.Y.Z
   ```

Released sections are not edited afterwards — they describe what shipped then.

## 3. Bump the version and build

```sh
cd devcrate
# version = "X.Y.Z" in Cargo.toml
cargo test                      # all green
cargo build --release           # zero warnings
target/release/devcrate.exe --version    # prints devcrate X.Y.Z
```

`cargo build` updates `Cargo.lock` to the new version; it is committed with
`Cargo.toml`. Run the network-bound test by hand too, since a release is when
a vendor's changed catalogue would hurt most:

```sh
cargo test -- --ignored offerings_from_the_live_catalogues --nocapture
```

The dashboard cannot be driven from a script ([it needs a real terminal](tui.md#the-terminal));
open `target/release/devcrate.exe` in Windows Terminal and press `i`, `U`,
and `?` before publishing, or say in the release notes that it was not done.

## 4. Commit the release

One commit, on `dev`, holding the changelog cut and the version bump only:

```
chore(release): vX.Y.Z

- Date the changelog's Unreleased section as X.Y.Z (YYYY-MM-DD) and open a fresh Unreleased

- Point the Unreleased compare link at vX.Y.Z and add the vPREV...vX.Y.Z compare link

- Bump the devcrate crate to X.Y.Z, so devcrate --version reports it
```

(Plus the `Co-Authored-By` trailer when an assistant made the commit.)

## 5. Package

The release zip is what a new user downloads instead of cloning: the stack's
configuration and `devcrate.exe`, in a `devcrate\` folder that is the stack
root once extracted. Built from the release commit, with the build from step 3:

```sh
scripts/package.sh X.Y.Z
```

It writes `dist/devcrate-X.Y.Z-windows-x64.zip` and its `.sha256`, and refuses
when the working tree has uncommitted changes, when `devcrate.exe --version` is
not `devcrate X.Y.Z`, or when the binary is older than the source. The zip is
`git archive HEAD` — so `.bat` files are CRLF whatever the checkout has — with
`devcrate.exe` added and these left out: the Rust source (`devcrate\`),
`scripts\`, the issue mirrors (`docs\issue\`), `.gitignore` and
`.gitattributes`, and `php\` — the `php.ini` files there are this repo's own
stack, and a new stack gets one generated for each version it installs, so it
lists only the PHP versions it has. The exclusions are pathspecs, not
`export-ignore`, which would also strip them from GitHub's own "Source code"
download. Last, it
unpacks the zip into a scratch folder and runs `devcrate status` against it.

`dist\` is gitignored.

## 6. Publish

Public from here on. Ask the owner first if you are not the owner.

```sh
git push origin dev
git branch -f main dev                     # fast-forward; refuses nothing, so step 1 matters
git push origin main                       # must report a fast-forward, e.g. 59f6f9b..ff1ce08
git tag -a vX.Y.Z <release-commit> -m "Devcrate vX.Y.Z - <summary>"
git push origin vX.Y.Z
```

Then the GitHub Release, with the zip, the bare binary (for updating a stack
that already exists), and a checksum for each:

```sh
cp devcrate/target/release/devcrate.exe dist/
(cd dist && sha256sum devcrate.exe | sed 's/\*//' > devcrate.exe.sha256)
gh release create vX.Y.Z \
  dist/devcrate-X.Y.Z-windows-x64.zip dist/devcrate-X.Y.Z-windows-x64.zip.sha256 \
  dist/devcrate.exe dist/devcrate.exe.sha256 \
  --title "Devcrate vX.Y.Z" --notes-file release-notes.md --verify-tag
```

`--verify-tag` makes `gh` refuse rather than create a tag of its own if the
push above did not land. If `gh` is not on `PATH` in an older shell, call
`"C:\Program Files\GitHub CLI\gh.exe"` directly.

### Release notes

Not the changelog again — it is linked, and it is long. The notes carry:

- One paragraph on what the release *is*, and a link to its changelog section
  (`CHANGELOG.md#XYZ---YYYY-MM-DD` at the `vX.Y.Z` tag, e.g.
  `#020---2026-10-15`).
- **Highlights** — a handful of bullets a user would act on.
- **Known limitations** — what is not done or not verified, in the same plain
  words the roadmap and the audits use. A release that hides a gap is the thing
  this project's docs exist to avoid.
- **Download** — the zip for a new stack (extract it, run `devcrate.exe`, press
  `s`), the bare binary for an existing one, and the sha256 of each.

## 7. Verify

```sh
gh release view vX.Y.Z                      # not a draft, not a pre-release, all four assets
gh release download vX.Y.Z -D check && (cd check && sha256sum -c *.sha256)
gh api repos/fadjardev/devcrate-stack/tags -q '.[].name'
```

The downloaded files' hashes must match the ones in the notes, and the
changelog's links at the `vX.Y.Z` tag must resolve.

## 8. Afterwards

- Copy the release build over the stack root's `devcrate.exe` (close the
  dashboard first — Windows will not replace a running executable), so the
  `devcrate` on `PATH` is the one just released.
- Keep logging under the new `[Unreleased]`.
