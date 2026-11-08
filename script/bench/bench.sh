#!/bin/bash
# Time until the first window is on screen and idle memory of a Mac app.
# Usage: script/bench/bench.sh <name> <path to the app's executable>
# Each run starts the app with an empty HOME (no projects), so apps are compared
# in the same state. The first run is a warm-up and is not counted. Memory is the
# physical footprint of the app's processes 10 s after the last start; pi and
# node processes the app starts are left out.
set -u
name=$1; exe=$2; runs=${RUNS:-6}
dir=$(cd "$(dirname "$0")" && pwd)
WINWAIT=${TMPDIR:-/tmp}/pi-gpui-bench/winwait
mkdir -p "$(dirname "$WINWAIT")"
[ -x "$WINWAIT" ] || swiftc -O "$dir/winwait.swift" -o "$WINWAIT"
home=${TMPDIR:-/tmp}/pi-gpui-bench/home-$name; rm -rf "$home"; mkdir -p "$home"
PI=$(command -v pi)
tree() { local p=$1; echo $p; for c in $(ps -axo pid=,ppid= | awk -v p=$p '$2==p{print $1}'); do tree $c; done; }
times=()
for i in $(seq 1 $runs); do
  env HOME="$home" PI_EXECUTABLE="$PI" PATH="$(dirname $PI):/usr/bin:/bin" "$exe" >/dev/null 2>&1 &
  pid=$!
  ms=$("$WINWAIT" $pid 60)
  [ $i -gt 1 ] && times+=($ms)
  if [ $i -eq $runs ]; then
    sleep 10
    pids=$(tree $pid)
    app=""; skipped=""
    for p in $pids; do c=$(ps -o comm= -p $p 2>/dev/null); case "$(basename "$c")" in node|pi) skipped="$skipped $p";; *) app="$app $p";; esac; done
    total=0; for p in $app; do b=$(footprint -f bytes $p 2>/dev/null | sed -n 's/.*Footprint: \([0-9]*\) B.*/\1/p' | head -1); total=$((total + ${b:-0})); done
    nproc=$(echo $app | wc -w | tr -d ' ')
    rss=0; for p in $app; do r=$(ps -o rss= -p $p 2>/dev/null); rss=$((rss + ${r:-0})); done
  fi
  for p in $(tree $pid | sort -rn); do kill $p 2>/dev/null; done
  sleep 3
  for p in $(tree $pid); do kill -9 $p 2>/dev/null; done
done
sorted=($(printf '%s\n' "${times[@]}" | sort -n)); med=${sorted[$(( ${#sorted[@]} / 2 ))]}
echo "$name | window ms: ${times[*]} | median $med | footprint $((total/1048576)) MB in $nproc processes (rss $((rss/1024)) MB) | skipped pi/node:$skipped"
