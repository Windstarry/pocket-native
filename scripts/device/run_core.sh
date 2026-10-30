#!/bin/bash
# run_core.sh <title id> [rom]: the run itself, started by run.sh either in
# place or, when the frontend is Sway, inside a systemd scope that survives
# the frontend being stopped. Sway holds the DRM master, and the KMS output
# cannot show a frame until it is gone (verified: the game renders at full
# speed with a pure black panel under sway, grim shows pure black) -- so sway
# is stopped for the run and everything it carried comes back afterwards.
dir="$(cd "$(dirname "$0")" && pwd)"
game="$dir/games/$1"
rom="${2:-$game/game.3ds}"
data="$game/data"

sw() { echo "$2" | sudo -n tee "$1" > /dev/null 2>&1; }
tune=${POCKET_NATIVE_TUNE:-1}
zram_mb=${POCKET_NATIVE_ZRAM_MB:-500}
zram=""
if [ "$tune" != 0 ]; then
    # compressed swap in RAM while the game runs (nothing is written to the
    # card), the device's own zram setup comes back afterwards
    if [ "$zram_mb" -gt 0 ]; then
        [ -e /sys/block/zram0 ] || sudo -n modprobe zram num_devices=1 2>/dev/null
        if grep -q '^/dev/zram' /proc/swaps && [ "$(awk '/^\/dev\/zram/ {s += $3} END {print int(s / 1024)}' /proc/swaps)" -ge "$zram_mb" ]; then
            :   # already there with that much or more
        elif [ -e /sys/block/zram0 ]; then
            zram=/sys/block/zram0
            zram_was_swap=0; grep -q '^/dev/zram0 ' /proc/swaps && zram_was_swap=1
            zram_old_size=$(cat $zram/disksize)
            zram_old_alg=$(sed -n 's/.*\[\(.*\)\].*/\1/p' $zram/comp_algorithm)
            [ $zram_was_swap = 1 ] && sudo -n swapoff /dev/zram0
            sw $zram/reset 1
            for alg in lz4 lzo-rle lzo; do
                grep -qw "$alg" $zram/comp_algorithm && sw $zram/comp_algorithm $alg && break
            done
            sw $zram/disksize "${zram_mb}M"
            sudo -n mkswap /dev/zram0 > /dev/null 2>&1 && sudo -n swapon -p 100 /dev/zram0
        fi
    fi
    # the file cache (the menus' files, the copy of the ROM) out of the way so
    # the game has the RAM to itself, and swap only when the RAM is really full
    sync
    sw /proc/sys/vm/drop_caches 3
    old_swappiness=$(cat /proc/sys/vm/swappiness 2>/dev/null)
    sw /proc/sys/vm/swappiness 10
    # top clocks from the start, the game's Power setting takes over once it runs
    "$dir/power.sh" performance
fi

# EmulationStation waits, stopped, while the game runs (systems without Sway;
# with Sway the stop of sway below takes the frontend down with it)
es=$(pgrep -f "^/usr/bin/emulationstation/emulationstation( |$)" || true)
[ -n "$es" ] && kill -STOP $es
sudo -n sh -c "echo 1 > /proc/sys/debug/exception-trace" 2>/dev/null

# free the DRM master: stopping sway also stops essway.service (Requires=),
# killing the process tree this port started from -- the systemd scope that
# runs this script is what keeps the game alive through that
sway_was=0
if command -v systemctl > /dev/null && systemctl is-active --quiet sway.service; then
    sway_was=1
    systemctl stop sway.service
    # a moment for the kernel to release the DRM master
    sleep 1
fi

restore() {
    if [ "$sway_was" = 1 ]; then
        systemctl start sway.service essway.service
    fi
    [ -n "$es" ] && kill -CONT $es 2>/dev/null
    sudo -n systemctl restart oga_events >/dev/null 2>&1 &
    if [ "$tune" != 0 ]; then
        "$dir/power.sh" normal
        [ -n "$old_swappiness" ] && sw /proc/sys/vm/swappiness "$old_swappiness"
        if [ -n "$zram" ]; then
            sudo -n swapoff /dev/zram0 2>/dev/null
            sw $zram/reset 1
            if [ "$zram_old_size" != 0 ]; then
                [ -n "$zram_old_alg" ] && sw $zram/comp_algorithm "$zram_old_alg"
                sw $zram/disksize "$zram_old_size"
                [ $zram_was_swap = 1 ] && sudo -n mkswap /dev/zram0 > /dev/null 2>&1 && sudo -n swapon /dev/zram0
            fi
        fi
    fi
}
trap restore EXIT

cd "$game"
status=0
while true; do
    "$dir/pocket-native" "$rom" > "$data/log.txt" 2>&1 &
    pid=$!
    [ -f "$dir/hotkey.py" ] && python3 "$dir/hotkey.py" "$pid" > /dev/null 2>&1 &
    hk_pid=$!
    # the game's threads ahead of everything else the system runs
    if [ "$tune" != 0 ]; then
        sleep 1
        for t in /proc/$pid/task/*; do sudo -n renice -n -10 -p "${t##*/}" > /dev/null 2>&1; done
    fi
    wait $pid
    status=$?
    [ -n "$hk_pid" ] && kill -9 "$hk_pid" 2>/dev/null
    [ $status -eq 3 ] || break
done
if [ $status -ne 0 ]; then
    echo "exit status $status" >> "$data/log.txt"
    dmesg | tail -40 >> "$data/log.txt"
fi
