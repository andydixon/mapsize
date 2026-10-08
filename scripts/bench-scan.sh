#!/usr/bin/env bash
# Records scan benchmarks as Markdown table rows for PERFORMANCE.md.
#
#   scripts/bench-scan.sh PATH [workers...]      (default workers: 1 2 4 8 16)
#
# Linux only (uses /usr/bin/time, df, lsblk). Runs each configuration three
# times against a warm cache; to measure a cold cache, run as root with
# DROP_CACHES=1, which drops the page cache before every run.
set -euo pipefail
path=${1:?usage: bench-scan.sh PATH [workers...]}
shift || true
workers=${*:-1 2 4 8 16}
bin=${MAPSIZE:-target/release/mapsize}
[[ -x $bin ]] || make build >/dev/null

fs=$(df -T "$path" | awk 'NR==2 {print $2}')
dev=$(df "$path" | awk 'NR==2 {print $1}')
medium=$(lsblk -no ROTA "$dev" 2>/dev/null | head -1 | sed 's/1/rotational/;s/0/solid-state/')
counts=$("$bin" --no-ui "$path" 2>/dev/null | awk '/^Files/ {f=$2} /^Directories/ {d=$2} END {print f" files, "d" dirs"}')

echo "<!-- $(date -u +%FT%TZ) $(uname -srm), $(nproc) CPUs -->"
echo "Path: \`$path\` · $fs on ${medium:-unknown medium} · $counts"
echo
echo "| workers | cache | wall (s) | peak RSS (MB) |"
echo "|---:|---|---:|---:|"
for w in $workers; do
  for i in 1 2 3; do
    cache=warm
    if [[ ${DROP_CACHES:-} == 1 ]]; then
      sync; echo 3 > /proc/sys/vm/drop_caches; cache=cold
    fi
    out=$( { /usr/bin/time -f "%e %M" "$bin" --no-ui --workers "$w" "$path" >/dev/null; } 2>&1 | tail -1)
    read -r secs kb <<<"$out"
    echo "| $w | $cache | $secs | $((kb / 1024)) |"
  done
done
