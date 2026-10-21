#!/bin/bash
# run.sh <title id> [rom]: plays a game Pocket Native was built with, from the
# launcher scripts/build.sh puts beside this folder, or from 3ds.sh with the
# ROM it was given. EmulationStation waits, stopped, while the game runs, and
# the game starts again when its menu's Reset asks for it.
# POCKET_NATIVE_TUNE=0 leaves the system alone, POCKET_NATIVE_ZRAM_MB=0 skips
# the compressed swap (500 MB otherwise).
dir="$(cd "$(dirname "$0")" && pwd)"
game="$dir/games/$1"
rom="${2:-$game/game.3ds}"
data="$game/data"
mkdir -p "$data" "$dir/config"

# one copy at a time
exec 9> /tmp/pocket-native.lock
if command -v flock > /dev/null && ! flock -n 9; then
    echo "already running"
    exit 1
fi

# the GPU's device nodes readable by the game, some systems keep them root-only
# and EGL then fails to start (the log says "drawing in software")
for node in /dev/dri/card0 /dev/dri/renderD128 /dev/mali0; do
    [ -e "$node" ] && { [ -r "$node" ] && [ -w "$node" ] || sudo -n chmod 666 "$node" 2> /dev/null; }
done

# Ensure libEGL.so.1 points to libMali.so if libMali is installed (prevents GLVND fallback)
if [ -e /usr/lib/aarch64-linux-gnu/libMali.so ]; then
    cur_egl=$(readlink -f /usr/lib/aarch64-linux-gnu/libEGL.so.1 2>/dev/null)
    mali_target=$(readlink -f /usr/lib/aarch64-linux-gnu/libMali.so 2>/dev/null)
    if [ "$cur_egl" != "$mali_target" ]; then
        sudo -n ln -sf libMali.so /usr/lib/aarch64-linux-gnu/libEGL.so.1 2>/dev/null
    fi
fi

# Expose log convenient symlinks
ln -sf "$data/log.txt" "$dir/../log.txt" 2>/dev/null
ln -sf "$data/log.txt" "$dir/log.txt" 2>/dev/null

export ZAKURO_FAST=1 ZAKURO_KMS=1 ZAKURO_DATA="$data" ZAKURO_RECOMPILED="$game/title.so" ZAKURO_SHADER_MANIFEST="$data/shaders.txt"
export POCKET_NATIVE_TITLES="$dir/titles" POCKET_NATIVE_POWER="$dir/power.sh" POCKET_NATIVE_CONFIG="$dir/config"
# draws to leave out besides the game's options, as ZAKURO_DROP_DRAWS takes them
[ -f "$data/drops.txt" ] && export ZAKURO_DROP_DRAWS="$(cat "$data/drops.txt")"

# The run itself goes through run_core.sh. When the frontend is Sway it starts
# in its own systemd scope: freeing the DRM master for the KMS output means
# stopping sway, and stopping sway stops essway.service, which owns this whole
# process tree when the port starts from EmulationStation -- the scope is what
# keeps the game and the cleanup alive through that. Without sway the scope is
# pointless, so run the core script in place as before.
if command -v systemctl > /dev/null && systemctl is-active --quiet sway.service; then
    systemd-run --scope --unit=pocket-native-run --collect --quiet bash "$dir/run_core.sh" "$@" && exit $?
fi
exec bash "$dir/run_core.sh" "$@"
