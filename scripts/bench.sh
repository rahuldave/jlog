#!/usr/bin/env bash
# Measures the time that jlog adds to each line of a recipe. It runs one recipe
# of N lines with `sh -cu`, then the same recipe with `jlog sh -cu`, in a
# temporary repository, and prints the extra time for each line.
set -euo pipefail

jlog="$(cd "$(dirname "$1")" && pwd)/$(basename "$1")"
n="${2:-500}"
rounds="${3:-10}"
dir="$(mktemp -d)"
trap 'rm -rf -- "$dir"' EXIT
mkdir -p "$dir/.agents"
git -C "$dir" init -q

body="$(for _ in $(seq "$n"); do printf '  true\n'; done)"
printf 'set shell := ["sh", "-cu"]\na:\n%s\n' "$body" > "$dir/plain.just"
printf 'set shell := ["%s", "sh", "-cu"]\na:\n%s\n' "$jlog" "$body" > "$dir/jlog.just"

# The bench runs as a terminal session, with a home folder of its own, so it
# reads no transcript and writes no key into the real home. The first line
# logs the session, the environment and the tools, and the best round hides it.
mkdir -p "$dir/home"
quiet_env=(env -u CODEX_THREAD_ID -u CLAUDE_CODE_SESSION_ID -u CLAUDE_CODE_CHILD_SESSION -u JLOG_SESSION "HOME=$dir/home" "JLOG_PLUGINS=$dir/no-plugins")

# The time of one run of a Justfile, in milliseconds.
once() {
    local t0 t1
    t0="$(perl -MTime::HiRes=time -e 'printf "%.6f", time')"
    "${quiet_env[@]}" just --quiet --justfile "$dir/$1" --working-directory "$dir" a >/dev/null
    t1="$(perl -MTime::HiRes=time -e 'printf "%.6f", time')"
    perl -e "printf '%.1f', ($t1 - $t0) * 1000"
}

# The rounds alternate, so that a change in the load of the machine hits both
# forms alike. Each form keeps its best round.
plain="" logged=""
for _ in $(seq "$rounds"); do
    p="$(once plain.just)"
    l="$(once jlog.just)"
    if [ -z "$plain" ] || perl -e "exit !($p < $plain)"; then plain="$p"; fi
    if [ -z "$logged" ] || perl -e "exit !($l < $logged)"; then logged="$l"; fi
done
lines="$(cat "$dir/.agents/log/just.jsonl" 2>/dev/null | wc -l | tr -d ' ' || true)"
perl -e "printf \"%d lines, best of %d rounds\nsh -cu:      %8.1f ms\njlog sh -cu: %8.1f ms\nextra time:  %8.3f ms for each line\n\", $n, $rounds, $plain, $logged, ($logged - $plain) / $n"
echo "run log lines written: $lines"
