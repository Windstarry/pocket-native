#!/bin/bash
# update.sh [--out dir] [--wait]: brings this kit up to the latest release of
# Pocket Native, then builds every game already in the output directory,
# build/ by default, again with it. The kit's old files go in .old. --check
# only says whether a new version is out, --wait waits for Enter at the end.
#
# the braces have bash read all of it before it runs, as it replaces itself
{
set -euo pipefail

repo="$(cd "$(dirname "$0")/.." && pwd)"
releases="${POCKET_NATIVE_RELEASES:-https://api.github.com/repos/whovian64/pocket-native/releases/latest}"
check="" out="$repo/build" wait=""
while [ $# -gt 0 ]; do
    case "$1" in
        --check) check=1; shift ;;
        --out) out="$(realpath -m "$2")"; shift 2 ;;
        --wait) wait=1; shift ;;
        -h|--help) sed -n '2,5p' "$0"; exit 0 ;;
        *) echo "usage: scripts/update.sh [--out dir]"; exit 2 ;;
    esac
done

finish() {
    local code=$?
    [ -n "$wait" ] && { read -rp "press Enter to close " _ || true; }
    exit $code
}
trap finish EXIT

current=$(sed -n 's/^version = "\(.*\)"$/\1/p' "$repo/Cargo.toml" | head -1)
if ! release=$(curl -fsSL -m 10 -H "User-Agent: pocket-native" "$releases" 2>/dev/null); then
    [ -n "$check" ] && exit 0
    echo "could not reach GitHub to look for a new version"
    exit 1
fi
field() { printf %s "$release" | grep -o "\"$1\": *\"[^\"]*\"" | head -1 | sed 's/.*: *"\(.*\)"/\1/'; }
latest=$(field tag_name) latest=${latest#v}
newer=""
[ -n "$latest" ] && [ "$latest" != "$current" ] && [ "$(printf '%s\n%s\n' "$current" "$latest" | sort -V | tail -1)" = "$latest" ] && newer=1

if [ -n "$check" ]; then
    [ -n "$newer" ] && echo "== Pocket Native $latest is out, you have $current. scripts/update.sh updates this kit and builds your games again."
    exit 0
fi
if [ -z "$newer" ]; then
    echo "this kit is up to date, Pocket Native $current"
    exit 0
fi

if [ -d "$repo/.git" ]; then
    echo "== updating this git clone to Pocket Native $latest"
    git -C "$repo" pull --ff-only
else
    echo "== downloading Pocket Native $latest"
    work="$repo/.update/kit"
    rm -rf "$work" && mkdir -p "$work"
    curl -fsSL -H "User-Agent: pocket-native" "$(field tarball_url)" | tar xz -C "$work"
    new=$(find "$work" -mindepth 1 -maxdepth 1 -type d | head -1)
    [ -f "$new/Cargo.toml" ] || { echo "the download is not a Pocket Native kit"; exit 1; }
    echo "== replacing this kit's files, the old ones go in .old"
    rm -rf "$repo/.old" && mkdir "$repo/.old"
    shopt -s dotglob
    for item in "$new"/*; do
        name=$(basename "$item")
        [ -e "$repo/$name" ] && mv "$repo/$name" "$repo/.old/"
        mv "$item" "$repo/"
    done
    rm -rf "$work"
fi

# the games already built, built again with the new kit
exec bash "$repo/scripts/batch-rebuild.sh" --out "$out" ${wait:+--wait}
}
