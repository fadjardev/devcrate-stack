#!/bin/sh
# Build the release zip: the stack's configuration plus devcrate.exe, in a
# devcrate\ folder that becomes the stack root when extracted.
#
#   scripts/package.sh 0.3.0
#
# Writes dist/devcrate-<version>-windows-x64.zip and its .sha256 sidecar, then
# unpacks the zip into a scratch folder and runs `devcrate status` against it.
# Run from Git Bash, after `cargo build --release` at the commit being released.
# See docs/releasing.md.
set -eu

version=${1:?usage: scripts/package.sh <version>, e.g. 0.3.0}
root=$(git rev-parse --show-toplevel)
cd "$root"

exe=devcrate/target/release/devcrate.exe
name=devcrate-$version-windows-x64
zip=dist/$name.zip

# The zip is HEAD's files, so anything uncommitted would be silently left out
# -- or, for the binary, silently built from something HEAD does not hold.
if [ -n "$(git status --porcelain --untracked-files=no)" ]; then
    echo "package: the working tree has uncommitted changes; commit or stash them first" >&2
    exit 1
fi
[ -f "$exe" ] || { echo "package: $exe not found; run cargo build --release in devcrate/" >&2; exit 1; }
built=$("$exe" --version)
if [ "$built" != "devcrate $version" ]; then
    echo "package: $exe reports \"$built\", not \"devcrate $version\"; bump Cargo.toml and rebuild" >&2
    exit 1
fi
if [ "$exe" -ot devcrate/Cargo.toml ] || [ -n "$(find devcrate/src -newer "$exe" -name '*.rs' | head -1)" ]; then
    echo "package: $exe is older than the source; run cargo build --release in devcrate/" >&2
    exit 1
fi

mkdir -p dist
rm -f "$zip" "$zip.sha256"

# git archive applies .gitattributes on the way out, so the .bat files are
# CRLF in the zip whatever the checkout has. Exclusions are pathspecs rather
# than export-ignore, which would also strip GitHub's own source download.
# php\ is left out: the php.ini files are this repo's own stack, and a new
# stack gets one generated for each version it actually installs.
git archive --format=zip --prefix=devcrate/ \
    --add-file="$exe" \
    -o "$zip" HEAD -- . \
    ':(exclude)devcrate' \
    ':(exclude)scripts' \
    ':(exclude)docs/issue' \
    ':(exclude)php' \
    ':(exclude).gitignore' \
    ':(exclude).gitattributes'

(cd dist && sha256sum "$name.zip" | sed 's/\*//' > "$name.zip.sha256")

# Smoke test: what a user gets when they extract it.
check=$(mktemp -d)
trap 'rm -rf "$check"' EXIT
unzip -q "$zip" -d "$check"
[ -f "$check/devcrate/devcrate.exe" ] || { echo "package: devcrate.exe is not at the zip's top level" >&2; exit 1; }
[ -f "$check/devcrate/nginx/conf/nginx.conf" ] || { echo "package: nginx\\conf\\nginx.conf is missing" >&2; exit 1; }
if unzip -l "$zip" | grep -q 'devcrate/devcrate/src'; then
    echo "package: the Rust source got into the zip" >&2
    exit 1
fi
"$check/devcrate/devcrate.exe" --root "$(cygpath -w "$check/devcrate")" status > /dev/null

echo "wrote $zip ($(du -h "$zip" | cut -f1)), $(unzip -Z1 "$zip" | grep -vc '/$') files"
cat "dist/$name.zip.sha256"
