#!/bin/sh
# [root] Check that refresh stops when its ext4 volume is unmounted during the run.
# The script makes a new 2 GiB image. It never accepts a device from the caller.
# It puts a 192 MiB file first in the scan list, and starts refresh. After 48 MiB of reads,
# it does a lazy unmount (umount -l). The mount point folder stays, but it is on another device.
# Refresh must stop with exit code 4 and the message "STOPPED: the volume is not available now".
# Then the script mounts the volume again, runs --resume, and compares all SHA-256 values.
# Usage: sh volume-loss-linux.sh <absolute CLI path> <new absolute report folder> [runs]
set -eu
export LC_ALL=C
if [ "$(id -u)" -ne 0 ]; then echo 'Run this test as root.' >&2; exit 1; fi
if [ "$#" -lt 2 ]; then echo 'Usage: sh volume-loss-linux.sh <CLI> <report folder> [runs]' >&2; exit 1; fi
cli=$1 reports=$2 runs=${3:-5}
case "$cli" in /*) ;; *) echo 'Use an absolute CLI path.' >&2; exit 1;; esac
case "$reports" in /*) ;; *) echo 'Use an absolute report path.' >&2; exit 1;; esac
test -x "$cli"
test ! -e "$reports"
for tool in losetup mkfs.ext4 mount umount mountpoint sha256sum dd awk; do
    command -v "$tool" >/dev/null || { echo "Missing command: $tool" >&2; exit 2; }
done
mkdir -p "$reports"
work=$(mktemp -d /tmp/ssd-refresh-loss.XXXXXX)
volume=$work/volume image=$work/disk.img loop=
cleanup() {
    result=$?
    trap - 0
    mountpoint -q "$volume" && umount "$volume"
    [ -n "$loop" ] && losetup -d "$loop"
    case "$work" in /tmp/ssd-refresh-loss.*) rm -rf -- "$work";; esac
    exit "$result"
}
trap cleanup 0
mkdir "$volume"
pass=0
n=1
while [ "$n" -le "$runs" ]; do
    r=$reports/run-$n
    mkdir "$r"
    rm -f "$image"
    truncate -s 2G "$image"
    loop=$(losetup --find --show --direct-io=on "$image")
    mkfs.ext4 -q -F -b 4096 "$loop"
    mount -t ext4 "$loop" "$volume"
    mkdir "$volume/data"
    dd if=/dev/urandom of="$volume/data/a-big.bin" bs=1048576 count=192 status=none
    i=0
    while [ "$i" -lt 10 ]; do
        dd if=/dev/urandom of="$volume/data/f$i.bin" bs=1048576 count=1 status=none
        i=$((i + 1))
    done
    sync
    (cd "$volume/data" && sha256sum ./*) > "$r/before.sha"
    "$cli" scan "$volume/data" --hash --parallel 1 --after-wait 0 --out "$r/scan.csv" > "$r/scan.log" 2>&1
    # Put the large file first. The other rows keep their order and text.
    { head -n 1 "$r/scan.csv"; grep "^$volume/data/a-big.bin," "$r/scan.csv"
      tail -n +2 "$r/scan.csv" | grep -v "^$volume/data/a-big.bin,"; } > "$r/scan.sorted"
    mv "$r/scan.sorted" "$r/scan.csv"
    code=0
    "$cli" refresh --from "$r/scan.csv" --select all --force --confirm "$volume" \
        --out "$r/refresh.csv" > "$r/refresh.log" 2>&1 &
    pid=$!
    cut=0
    t=0
    # Count the reads of the loop device. /proc/<pid>/io does not count io_uring reads.
    stat=/sys/block/${loop#/dev/}/stat
    start=$(awk '{print $3}' "$stat")
    while kill -0 "$pid" 2>/dev/null && [ "$t" -lt 2400 ]; do
        sectors=$(($(awk '{print $3}' "$stat") - start))
        if [ "$sectors" -ge 98304 ]; then
            umount -l "$volume"
            cut=$((sectors / 2048))
            break
        fi
        sleep 0.05
        t=$((t + 1))
    done
    wait "$pid" || code=$?
    rows=$(($(wc -l < "$r/refresh.csv") - 1))
    first=$(awk -F, 'NR==2 {print $3}' "$r/refresh.csv")
    stop=NO
    grep -q 'STOPPED: the volume is not available now' "$r/refresh.log" && stop=YES
    # Mount again at the same path. --resume needs the paths of scan.csv.
    mountpoint -q "$volume" && umount "$volume"
    mount -t ext4 "$loop" "$volume"
    resume=0
    "$cli" refresh --from "$r/scan.csv" --select all --force --confirm "$volume" \
        --out "$r/refresh.csv" --resume > "$r/resume.log" 2>&1 || resume=$?
    big=$(awk -F, -v p="$volume/data/a-big.bin" '$1==p {s=$3} END {print s}' "$r/refresh.csv")
    (cd "$volume/data" && sha256sum ./*) > "$r/after.sha"
    data=NO
    cmp -s "$r/before.sha" "$r/after.sha" && data=YES
    umount "$volume"
    fsck=0
    e2fsck -fn "$loop" > "$r/fsck.txt" 2>&1 || fsck=$?
    losetup -d "$loop"
    loop=
    loss=NO
    if [ "$cut" -gt 0 ] && [ "$code" -eq 4 ] && [ "$stop" = YES ] && [ "$resume" -eq 0 ] && [ "$big" = OK ]; then loss=YES; fi
    echo "RESULT run-$n: loss $loss data $data fsck_exit=$fsck cut_mb=$cut exit=$code rows=$rows first=$first stop_msg=$stop resume_exit=$resume big=$big"
    if [ "$loss" = YES ] && [ "$data" = YES ] && [ "$fsck" -eq 0 ]; then pass=$((pass + 1)); fi
    n=$((n + 1))
done
echo "VOLUME LOSS LINUX: $pass of $runs YES"
[ "$pass" -eq "$runs" ]
