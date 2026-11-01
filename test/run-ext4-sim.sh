#!/bin/sh
# Use only a new image. Never accept a device from the caller.
set -eu
export LC_ALL=C
if [ "$(id -u)" -ne 0 ]; then echo 'Run this test as root.' >&2; exit 1; fi
if [ "$#" -ne 2 ]; then
    echo 'Usage: sh run-ext4-sim.sh <absolute CLI path> <new absolute report folder>' >&2
    exit 1
fi
cli=$1 reports=$2
case "$cli" in /*) ;; *) echo 'Use an absolute CLI path.' >&2; exit 1;; esac
case "$reports" in /*) ;; *) echo 'Use an absolute report path.' >&2; exit 1;; esac
test -x "$cli"
test ! -e "$reports"
test ! -L "$reports"
for tool in dmsetup losetup blockdev mkfs.ext4 filefrag mount umount mountpoint timeout awk sha256sum fallocate; do
    command -v "$tool" >/dev/null || { echo "Missing command: $tool" >&2; exit 2; }
done
# The outer process gives cleanup 20 seconds after the 14-minute limit.
if [ "${SSD_REFRESH_SIM_TIMED:-0}" != 1 ]; then
    code=0
    SSD_REFRESH_SIM_TIMED=1 timeout -s TERM -k 20 840 sh "$0" "$@" || code=$?
    if [ "$code" -eq 124 ] || [ "$code" -eq 137 ]; then
        echo 'EXT4 SIM: NO (time limit)'
    fi
    exit "$code"
fi
if ! dmsetup targets 2>/dev/null | grep -q '^delay '; then
    modprobe dm_delay 2>/dev/null || :
    if ! dmsetup targets 2>/dev/null | grep -q '^delay '; then
        echo 'The Linux kernel needs the device-mapper delay target (dm_delay). Test not run.' >&2
        exit 2
    fi
fi
mkdir -p "$reports"
work=$(mktemp -d /tmp/ssd-refresh-sim.XXXXXX)
volume=$work/volume image=$work/disk.img
name=ssd-refresh-sim-$$
device=/dev/mapper/$name
loop= mounted=0 mapped=0
deadline=$(($(date +%s) + 840))
cleanup_device() {
    if [ "$mapped" -eq 1 ]; then dmsetup resume "$name" || return 1; fi
    if [ "$mounted" -eq 1 ]; then
        umount "$volume" || return 1
        mounted=0
    fi
    if [ "$mapped" -eq 1 ]; then
        dmsetup remove "$name" || return 1
        mapped=0
    fi
    if [ -n "$loop" ]; then
        losetup -d "$loop" || return 1
        loop=
    fi
}
cleanup() {
    result=$?
    trap - 0
    if ! cleanup_device; then
        echo "Cleanup failed. Keep the image: $image" >&2
        exit 1
    fi
    case "$work" in /tmp/ssd-refresh-sim.*) rm -rf -- "$work";; *) exit 1;; esac
    exit "$result"
}
trap cleanup 0
trap 'exit 130' INT
trap 'exit 143' TERM
mkdir "$volume"

# These synthetic paths have no commas or newlines. Other CSV fields can have quotes.
# Read columns by name, including quoted fields and doubled quotes.
cat > "$work/csv.awk" <<'AWK'
function parse(line, a,    i,c,q,n,s) {
    n=1; s=""; q=0
    for(i=1;i<=length(line);i++) {
        c=substr(line,i,1)
        if(c=="\"") {
            if(q && substr(line,i+1,1)=="\"") { s=s c; i++ }
            else q=!q
        } else if(c=="," && !q) { a[n++]=s; s="" }
        else s=s c
    }
    a[n]=s; return n
}
{ sub(/\r$/, "") }
NR==1 { sub(/^\357\273\277/, ""); n=parse($0,a); for(i=1;i<=n;i++) h[a[i]]=i; next }
{ parse($0,a); n=split(cols,k,","); for(i=1;i<=n;i++) {
    if(!(k[i] in h)) exit 2
    printf "%s%s", a[h[k[i]]], (i==n ? "\n" : "|")
} }
AWK
rows() { awk -v cols="$2" -f "$work/csv.awk" "$1"; }
run_cli() {
    log=$1; shift
    remaining=$((deadline - $(date +%s)))
    if [ "$remaining" -le 0 ]; then echo 'Test time limit reached.' >&2; return 1; fi
    code=0
    timeout -s TERM -k 10 "$remaining" "$cli" "$@" > "$log" 2>&1 || code=$?
    printf '%s\n' "$code" > "$log.exit"
    if [ "$code" -ne 0 ]; then cat "$log"; return 1; fi
}
linear() { printf '0 %s linear %s 0\n' "$sectors" "$loop" > "$work/table"; }
reload() {
    sync
    cp "$work/table" "$r/table-$table_id.txt"
    table_id=$((table_id + 1))
    dmsetup suspend "$name"
    # Always attempt resume, also if load fails.
    load_code=0
    dmsetup load "$name" "$work/table" || load_code=$?
    dmsetup resume "$name"
    test "$load_code" -eq 0
}
fresh() {
    cleanup_device
    truncate -s 0 "$image"
    truncate -s 1G "$image"
    # Direct I/O keeps the host page cache out of the speed. Without it, recently
    # written image blocks read from memory and look fast.
    loop=$(losetup --find --show --direct-io=on "$image")
    test "$(cat "/sys/block/${loop#/dev/}/loop/dio")" = 1
    sectors=$(blockdev --getsz "$loop")
    linear
    dmsetup create "$name" "$work/table"
    mapped=1
    mkfs.ext4 -q -F -b 4096 "$device"
    mount -t ext4 "$device" "$volume"
    mounted=1
    mkdir "$volume/data"
    table_id=0
}
make_file() { dd if=/dev/urandom of="$1" bs=1048576 count="${2:-4}" status=none; }
ranges() {
    : > "$work/ranges"
    for f do
        filefrag -e -b4096 "$f" >> "$r/extents.log"
        filefrag -e -b4096 "$f" | awk -F'[: .]+' '
            /^ *[0-9]+:/ { print $5*8, ($6-$5+1)*8; n++ }
            END { if(!n) exit 1 }' >> "$work/ranges"
    done
    sort -n -k1,1 "$work/ranges" > "$work/sorted"
    # Reject overlaps and invalid ranges before the table can reach the kernel.
    awk -v end="$sectors" '($1<p || $2<=0 || $1+$2>end){exit 1} {p=$1+$2}' "$work/sorted"
}
delay_ranges() {
    awk -v dev="$loop" -v end="$sectors" -v ms="$delay" '
        BEGIN { p=0 }
        $1>p { print p, $1-p, "linear", dev, p }
        { print $1, $2, "delay", dev, $1, ms, dev, $1, 0; p=$1+$2 }
        END { if(p<end) print p, end-p, "linear", dev, p }
    ' "$work/sorted" > "$work/table"
    reload
}
# Slowness follows old data, not the address. A move frees slow blocks, and
# the next move can put new data there. Thus, after refresh, delay only the
# blocks of the files that did not move.
delay_files() {
    if [ "$#" -eq 0 ]; then linear; reload; return; fi
    ranges "$@"
    delay_ranges
}
scan() {
    run_cli "$r/$1.log" scan "$volume/data" --hash --parallel 1 --after-wait 0 \
        --threshold "$threshold" --allowance-ms 0 --qd 1 --out "$r/$1.csv"
}
refresh() {
    run_cli "$r/refresh.log" refresh --from "$r/scan.csv" --threshold "$threshold" \
        --allowance-ms 0 --qd 1 --confirm "$volume" --out "$r/refresh.csv" "$@"
}
verify() {
    run_cli "$r/verify.log" verify --from "$r/refresh.csv" --threshold "$threshold" \
        --allowance-ms 0 --qd 1 --out "$r/verify.csv"
    grep -Eq 'Data safe: +YES' "$r/verify.log"
    grep -Eq 'Faster.*NOT DECIDED' "$r/verify.log"
    grep -Fq '12 hours after the refresh' "$r/verify.log"
    cat "$r/refresh.log" "$r/verify.log"
}
check_scan() {
    rows "$r/$1.csv" 'path,status,slow,sha256' > "$r/$1.rows"
    awk -F'|' -v total="$2" -v slow="$3" '
        {n++; s+=$3; if($2!="OK" || length($4)!=64) bad=1}
        END {exit (bad || n!=total || (slow>=0 && s!=slow))}' "$r/$1.rows"
}
hashes() { sha256sum -c "$r/before.sha256" > "$r/hashes.log"; }
save_hashes() { sha256sum "$volume"/data/*.bin > "$r/before.sha256"; }

# Calibrate with the same read options as the scenarios. Use read-only delay.
r=$reports/calibration
mkdir "$r"
fresh
make_file "$volume/data/fast.bin"
make_file "$volume/data/slow.bin"
sync
threshold=100
scan linear
check_scan linear 2 -1
fast=$(rows "$r/linear.csv" 'min_extent_mbps' | sort -n | head -n 1)
threshold=$(awk -v f="$fast" 'BEGIN {t=f/3; if(t>300)t=300; printf "%.3f",t}')
ranges "$volume/data/slow.bin"
calibrated=0
for delay in 1 2 4 8 16 32; do
    delay_ranges
    scan "delay-$delay"
    slow=$(rows "$r/delay-$delay.csv" 'path,mbps' | awk -F'|' '/\/slow.bin\|/ {print $2}')
    normal=$(rows "$r/delay-$delay.csv" 'path,min_extent_mbps' | awk -F'|' '/\/fast.bin\|/ {print $2}')
    printf 'CAL: delay=%s ms linear=%s MB/s slow=%s MB/s normal=%s MB/s threshold=%s MB/s\n' \
        "$delay" "$fast" "$slow" "$normal" "$threshold" | tee -a "$r/result.log"
    if awk -v s="$slow" -v f="$normal" -v t="$threshold" 'BEGIN {exit !(s>0 && s<t*0.7 && f>t*2)}'; then
        check_scan "delay-$delay" 2 1
        calibrated=1
        break
    fi
done
if [ "$calibrated" -ne 1 ]; then echo 'Calibration failed. No test result.' >&2; exit 1; fi
linear; reload

scenario_a() {
    # Ask the real CLI for its path groups. Keep four paths from each group.
    i=1
    while [ "$i" -le 64 ]; do make_file "$volume/data/old-$i.bin"; i=$((i+1)); done
    # Without sync, ext4 has not allocated blocks yet and scan skips the files.
    sync
    scan candidates
    run_cli "$r/groups.log" refresh --from "$r/candidates.csv" --select all --force \
        --dry-run --control-fraction 0.5 --out "$r/groups.csv"
    rows "$r/groups.csv" 'path,status' > "$r/groups.rows"
    awk -F'|' '{n[$2]++; if(($2=="CONTROL" || $2=="DRYRUN") && n[$2]<=4) print $1}' \
        "$r/groups.rows" > "$work/keep"
    test "$(wc -l < "$work/keep")" -eq 8
    for f in "$volume"/data/*.bin; do grep -Fxq "$f" "$work/keep" || rm -- "$f"; done
    i=1
    while [ "$i" -le 8 ]; do make_file "$volume/data/new-$i.bin"; i=$((i+1)); done
    sync; save_hashes
    ranges "$volume"/data/old-*.bin
    delay_ranges
    scan scan
    check_scan scan 16 8
    awk -F'|' '{if(($1 ~ /\/old-/ && $3!=1) || ($1 ~ /\/new-/ && $3!=0)) exit 1}' "$r/scan.rows"
    refresh --control-fraction 0.5
    rows "$r/refresh.csv" 'path,status,moved_bytes' > "$r/refresh.rows"
    awk -F'|' '{n++; c+=($2=="CONTROL"); m+=($2=="OK" && $3==4194304)} END {exit !(n==8 && c==4 && m==4)}' "$r/refresh.rows"
    # shellcheck disable=SC2046 # Synthetic paths have no spaces.
    delay_files $(awk -F'|' '$2=="CONTROL" {print $1}' "$r/refresh.rows")
    scan after
    check_scan after 16 4
    awk -F'|' 'NR==FNR {s[$1]=$2; next}
        (s[$1]=="OK" && $3!=0) || (s[$1]=="CONTROL" && $3!=1) {exit 1}' "$r/refresh.rows" "$r/after.rows"
    hashes
    verify
    echo 'slow_before=8 moved=4 control=4 slow_after=4 verify_exit=0 data_safe=YES speed=NOT_DECIDED'
}
scenario_b() {
    i=1
    while [ "$i" -le 8 ]; do make_file "$volume/data/new-$i.bin"; i=$((i+1)); done
    sync; save_hashes
    scan scan
    check_scan scan 8 0
    refresh
    # Refresh returns before it creates a CSV when the selection is empty.
    test ! -e "$r/refresh.csv"
    grep -Fq 'There is nothing to do.' "$r/refresh.log"
    hashes
    echo 'slow=0 moved=0 delay_targets=0'
}
scenario_c() {
    make_file "$volume/data/old.bin"
    sync; save_hashes
    printf '0 %s delay %s 0 %s %s 0 0\n' "$sectors" "$loop" "$delay" "$loop" > "$work/table"
    reload
    scan scan
    check_scan scan 1 1
    refresh
    rows "$r/refresh.csv" 'status,moved_bytes,after_slow' > "$r/refresh.rows"
    test "$(cat "$r/refresh.rows")" = 'OK|4194304|1'
    scan after
    check_scan after 1 1
    hashes
    verify
    if grep -Eq 'Faster[^:]*: +YES|Data safe: +NO' "$r/verify.log"; then return 1; fi
    echo 'slow_before=1 moved=1 slow_after=1 verify_exit=0 data_safe=YES speed=NOT_DECIDED'
}
scenario_d() {
    i=1
    while [ "$i" -le 8 ]; do make_file "$volume/data/old-$i.bin"; i=$((i+1)); done
    sync; save_hashes
    ranges "$volume"/data/*.bin; delay_ranges
    scan scan
    check_scan scan 8 8
    ranges "$volume/data/old-5.bin" "$volume/data/old-6.bin" "$volume/data/old-7.bin" "$volume/data/old-8.bin"
    delay_ranges
    # Refresh reads again before the move. Fast files get SKIP_FAST before control selection.
    # See src/refresh/file.rs: the pre.is_slow() check precedes ops.relocate().
    refresh
    rows "$r/refresh.csv" 'path,status,moved_bytes' > "$r/refresh.rows"
    awk -F'|' '{n++; if($1 ~ /old-[1-4]\.bin$/) {if($2!="SKIP_FAST" || $3!=0) bad=1; k++}
        else {if($2!="OK" || $3!=4194304) bad=1; m++}}
        END {exit (bad || n!=8 || k!=4 || m!=4)}' "$r/refresh.rows"
    # Files 1-4 were already fast and files 5-8 moved. No old slow data is left.
    delay_files
    scan after
    check_scan after 8 0
    hashes
    echo 'slow_before=8 repaired_by_read=4 SKIP_FAST=4 moved=4 slow_after=0'
}
scenario_e() {
    make_file "$volume/data/contiguous.bin" 32
    sync
    # Release 272 separate 128 KiB holes. Leave 2 MiB for extent tree blocks.
    # This prevents ext4 allocation ahead of the writes from joining the test extents.
    fallocate -l 71303168 "$volume/arena.bin"
    fill_code=0
    dd if=/dev/zero of="$volume/filler.bin" bs=4096 status=none 2> "$r/fill.log" || fill_code=$?
    test "$fill_code" -ne 0
    grep -Fq 'No space left on device' "$r/fill.log"
    sync
    i=0
    while [ "$i" -lt 272 ]; do
        fallocate --punch-hole --keep-size -o "$((i * 262144))" -l 131072 "$volume/arena.bin"
        i=$((i+1))
    done
    make_file "$volume/data/fragmented.bin" 32
    sync
    filefrag -e -b4096 "$volume"/data/*.bin > "$r/extents.log"
    extents=$(filefrag -e -b4096 "$volume/data/fragmented.bin" | awk '/^ *[0-9]+:/ {n++} END {print n+0}')
    contiguous=$(filefrag -e -b4096 "$volume/data/contiguous.bin" | awk '/^ *[0-9]+:/ {n++} END {print n+0}')
    # Fixture requirements only. Do not assert a speed or a ratio.
    # ext4 can join some holes. 128 or more extents is enough for this measurement.
    test "$extents" -ge 128
    test "$contiguous" -eq 1
    # The host can cache the image of a container disk, and the last written file
    # then reads from memory. A fixed 1 ms delay on each read makes the request
    # count set the time. This is the cost of the queue drain between extents.
    printf '0 %s delay %s 0 1 %s 0 0\n' "$sectors" "$loop" "$loop" > "$work/table"
    reload
    # Use four requests to include the queue drain cost between extents.
    run_cli "$r/scan.log" scan "$volume/data" --hash --parallel 1 --after-wait 0 \
        --threshold "$threshold" --allowance-ms 0 --qd 4 --out "$r/scan.csv"
    check_scan scan 2 -1
    # Scan again with the default extra time per extent (no --allowance-ms).
    run_cli "$r/default.log" scan "$volume/data" --hash --parallel 1 --after-wait 0 \
        --threshold "$threshold" --qd 4 --out "$r/default.csv"
    rows "$r/scan.csv" 'path,mbps,slow' > "$r/speeds.rows"
    rows "$r/default.csv" 'path,slow' > "$r/default.rows"
    d=$(awk -F'|' '/\/fragmented.bin\|/ {print $2}' "$r/default.rows")
    awk -F'|' -v n="$extents" -v d="$d" '/\/contiguous.bin\|/ {c=$2} /\/fragmented.bin\|/ {f=$2; s=$3}
        END {if(c<=0 || f<=0) exit 1; printf "contiguous=%.3f MB/s fragmented=%.3f MB/s ratio=%.3f extents=%d fragmented_slow_allowance0=%s fragmented_slow_default=%s\n",c,f,c/f,n,s,d}' "$r/speeds.rows"
}

failed=
# SSD_REFRESH_SIM_ONLY="A D" runs only some scenarios (for debugging).
for scenario in ${SSD_REFRESH_SIM_ONLY:-A B C D E}; do
    r=$reports/$scenario
    mkdir "$r"
    # A child shell keeps errexit active inside each scenario.
    # The parent owns the device and mount. Children change only the table and files.
    fresh
    case "$scenario" in A) fn=scenario_a;; B) fn=scenario_b;; C) fn=scenario_c;; D) fn=scenario_d;; E) fn=scenario_e;; esac
    # Run in this shell: using an if on a function would disable errexit.
    # A separate result marker distinguishes an assertion failure from success.
    marker=$r/passed
    set +e
    (trap - 0 INT TERM; set -e; "$fn"; touch "$marker") > "$r/scenario.log" 2>&1
    code=$?
    set -e
    cat "$r/scenario.log"
    if [ "$code" -eq 0 ] && [ -f "$marker" ]; then
        printf 'SIM %s: YES %s\n' "$scenario" "$(tail -n 1 "$r/scenario.log")" | tee "$r/result.log"
    else
        failed="$failed $scenario"
        printf 'SIM %s: NO exit=%s\n' "$scenario" "$code" | tee "$r/result.log"
    fi
    cleanup_device
done
if [ -z "$failed" ]; then
    echo 'EXT4 SIM: YES (5 scenarios)' | tee "$reports/result.log"
else
    printf 'EXT4 SIM: NO (%s)\n' "${failed# }" | tee "$reports/result.log"
    exit 1
fi
