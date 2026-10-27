#!/bin/bash
# build.sh [--out dir] [--test]: builds Pocket Native's Android app,
# pocket-native.apk in the output directory (build/ by default), inside a
# container (podman or docker). --test makes io.github.pocketnative.test,
# signed with a test key, which installs beside the real app. The real app is
# signed with the keystore in KEYSTORE (its password in KEYSTORE_PASS), kept
# by whoever releases it.
set -euo pipefail

repo="$(cd "$(dirname "$0")/../.." && pwd)"
out="$repo/build" test=0
while [ $# -gt 0 ]; do
    case "$1" in
        --out) out="$(realpath -m "$2")"; shift 2 ;;
        --test) test=1; shift ;;
        -h|--help) sed -n '2,8p' "$0"; exit 0 ;;
        *) echo "unknown option $1"; exit 2 ;;
    esac
done
engine=$(command -v podman || command -v docker) || { echo "the build needs podman or docker"; exit 1; }
image=pocket-native-android:$(sha256sum "$repo/scripts/android/Containerfile" | cut -c1-12)
if ! "$engine" image inspect "$image" >/dev/null 2>&1; then
    echo "== making the Android build container, once (the NDK is about 1 GB)"
    "$engine" build -f "$repo/scripts/android/Containerfile" -t "$image" "$repo/scripts/android"
fi
mkdir -p "$out/.work-android"
user=(--user "$(id -u):$(id -g)")
[ "$(basename "$engine")" = podman ] && user=(--userns=keep-id)
mounts=(-v "$repo:/src:ro" -v "$out/.work-android:/work" -v "$out:/out")
if [ $test = 1 ]; then
    args=(/out/pocket-native-test.apk /work/test.keystore io.github.pocketnative.test)
else
    [ -n "${KEYSTORE:-}" ] && [ -f "$KEYSTORE" ] || { echo "KEYSTORE names the release keystore; --test builds without it"; exit 2; }
    mounts+=(-v "$(dirname "$(realpath "$KEYSTORE")"):/keys")
    args=(/out/pocket-native.apk "/keys/$(basename "$KEYSTORE")")
fi
"$engine" run --rm "${user[@]}" "${mounts[@]}" -e CARGO_HOME=/work/cargo -e CARGO_TARGET_DIR=/work/target -e HOME=/work \
    -e KEYSTORE_PASS="${KEYSTORE_PASS:-android}" "$image" bash /src/scripts/android/inside.sh "${args[@]}"
echo "== $out/$(basename "${args[0]}")"
