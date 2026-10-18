#!/bin/sh
# Use a new loop image. Never accept an existing device or mount point.
set -eu
if [ "$(id -u)" -ne 0 ]; then
    echo 'Run this test as root.' >&2
    exit 1
fi
if [ "$#" -ne 2 ]; then
    echo 'Usage: sh run-ext4-test.sh <absolute CLI path> <new report folder>' >&2
    exit 1
fi
cli=$1
reports=$2
case "$cli" in /*) ;; *) echo 'Use an absolute CLI path.' >&2; exit 1;; esac
case "$reports" in /*) ;; *) echo 'Use an absolute report path.' >&2; exit 1;; esac
test -x "$cli"
test ! -e "$reports"
mkdir -p "$reports"
work=$(mktemp -d /tmp/ssd-refresh-ext4.XXXXXX)
image=$work/disk.img
volume=$work/volume
loop=
mounted=0
cleanup() {
    result=$?
    trap - EXIT
    if [ "$mounted" -eq 1 ]; then
        if ! umount "$volume"; then
            echo "Unmount failed. Keep the test image: $work" >&2
            exit 1
        fi
    fi
    if [ -n "$loop" ] && ! losetup -d "$loop"; then
        echo "Detach failed. Keep the test image: $work" >&2
        exit 1
    fi
    case "$work" in /tmp/ssd-refresh-ext4.*) rm -rf -- "$work";; *) exit 1;; esac
    exit "$result"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
truncate -s 256M "$image"
mkfs.ext4 -q -F -b 4096 "$image"
mkdir "$volume"
loop=$(losetup --find --show "$image")
mount -t ext4 "$loop" "$volume"
mounted=1
test "$(findmnt -n -o SOURCE --target "$volume")" = "$loop"
test "$(findmnt -n -o FSTYPE --target "$volume")" = ext4
mkdir "$volume/data"
index=1
while [ "$index" -le 8 ]; do
    file=$volume/data/file-$index.bin
    dd if=/dev/urandom of="$file" bs=1048576 count=2 2>/dev/null
    chmod 640 "$file"
    sha256sum "$file" >> "$reports/before.sha256"
    stat -c '%n %i %s %a %u %g %y' "$file" >> "$reports/before.metadata"
    index=$((index + 1))
done
sync
"$cli" volume "$volume/data" > "$reports/volume.log"
"$cli" scan "$volume/data" --hash --out "$reports/scan.csv" > "$reports/scan.log"
"$cli" extents "$volume/data/file-1.bin" > "$reports/extents-before.log"
result=0
"$cli" refresh --from "$reports/scan.csv" --select all --force --confirm "$volume" --out "$reports/refresh.csv" --stop-after 2 > "$reports/stop.log" || result=$?
test "$result" -eq 4
awk -F, 'NR > 1 { if ($3 != "OK" || $8 != 2097152 || $9 < 1 || $10 != 0) exit 1; count++ } END { if (count != 2) exit 1 }' "$reports/refresh.csv"
"$cli" refresh --from "$reports/scan.csv" --select all --force --confirm "$volume" --out "$reports/refresh.csv" --resume > "$reports/resume.log"
awk -F, 'NR > 1 { if ($3 != "OK" || $8 != 2097152 || $9 < 1 || $10 != 0 || seen[$1]++) exit 1; count++ } END { if (count != 8) exit 1 }' "$reports/refresh.csv"
"$cli" extents "$volume/data/file-1.bin" > "$reports/extents-after.log"
if cmp -s "$reports/extents-before.log" "$reports/extents-after.log"; then
    echo 'The test file did not move.' >&2
    exit 1
fi
"$cli" verify --from "$reports/refresh.csv" --out "$reports/verify.csv" > "$reports/verify.log"
grep -q 'Data safe: YES 8 of 8' "$reports/verify.log"
sha256sum -c "$reports/before.sha256" > "$reports/hash.log"
index=1
while [ "$index" -le 8 ]; do
    stat -c '%n %i %s %a %u %g %y' "$volume/data/file-$index.bin" >> "$reports/after.metadata"
    index=$((index + 1))
done
cmp "$reports/before.metadata" "$reports/after.metadata"
sync
umount "$volume"
mounted=0
e2fsck -f -n "$loop" > "$reports/e2fsck.log" 2>&1
printf '%s\n' 'EXT4 TEST: YES (8 files, stop, resume, move, hashes, metadata, filesystem)' | tee "$reports/result.log"
