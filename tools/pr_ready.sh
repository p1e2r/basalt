#!/usr/bin/env bash
# Run before merging: formatting, clippy (new warnings only), unit tests, and the affected harnesses.
#
# usage: tools/pr_ready.sh [--client] [--all] [--no-vanilla] [--update-baselines] [--base REF]
#   --client           also run the client harness group (`basalt-check --group client --affected`);
#                      skipped with a note while no client harnesses are registered
#   --all              run every harness instead of only those affected by the diff against --base
#   --no-vanilla       don't use the vanilla test server (differential/fuzz harnesses are skipped)
#   --update-baselines rewrite tools/baselines/* from the current tree (do this only to ACCEPT the
#                      current warnings / unformatted files; reviewers should see the diff)
#   --base REF         base for "affected" (default: merge-base with main)
# Gates:
#   fmt      `cargo fmt --check`: the tree predates rustfmt (dense one-line style, ~100 files differ), so only
#            files NOT listed in tools/baselines/fmt-dirty.txt must be clean (new files are not exempt).
#   clippy   `cargo clippy --workspace --all-targets`: warnings AND errors are compared with tools/baselines/clippy.txt
#            (counted per file + message); only NEW ones fail. No lint is allowed away. (The baseline holds one
#            deny-level error in crates/client/src/scenario.rs, `erasing_op`, so clippy cannot finish that crate.)
#   tests    `cargo test --workspace`
#   harness  `basalt-check --affected` (scenarios + fuzz + parity whose declared paths changed; results are
#            cached in target/check/cache.json, see crates/check/src/harness.rs)
# Exit status is non-zero on any failure; a pass/fail table is printed at the end.
set -uo pipefail
cd "$(dirname "$0")/.."
CLIENT=0; ALL=0; VANILLA=1; UPDATE=0; BASE=""
while [ $# -gt 0 ]; do
  case "$1" in
    --client) CLIENT=1 ;; --all) ALL=1 ;; --no-vanilla) VANILLA=0 ;; --update-baselines) UPDATE=1 ;;
    --base) BASE=$2; shift ;;
    -h|--help) sed -n 2,24p "$0"; exit 0 ;;
    *) echo "unknown option $1"; exit 2 ;;
  esac
  shift
done
OUT=target/check; BASELINES=tools/baselines
mkdir -p "$OUT" "$BASELINES"
ROWS="$OUT/pr_ready.rows"; : > "$ROWS"
now() { python3 -c 'import time; print(time.time())'; }
since() { python3 -c "import time; print(round(time.time()-$1,1))"; }
row() { printf '%s\t%s\t%s\t%s\n' "$1" "$2" "$3" "$4" >> "$ROWS"; }   # name result secs detail
FAIL=0

# ------------------------------------------------------------------ fmt
T=$(now)
cargo fmt --check > "$OUT/fmt.log" 2>&1
grep '^Diff in ' "$OUT/fmt.log" | sed -E 's/^Diff in //; s/:[0-9]+:?$//' | sed "s#^$PWD/##" | sort -u > "$OUT/fmt-dirty.now"
if [ $UPDATE = 1 ]; then cp "$OUT/fmt-dirty.now" "$BASELINES/fmt-dirty.txt"; fi
[ -f "$BASELINES/fmt-dirty.txt" ] || : > "$BASELINES/fmt-dirty.txt"
NEWDIRTY=$(comm -23 "$OUT/fmt-dirty.now" <(sort -u "$BASELINES/fmt-dirty.txt"))
if [ -z "$NEWDIRTY" ]; then
  row "cargo fmt --check" pass "$(since $T)" "no new unformatted files ($(wc -l < "$BASELINES/fmt-dirty.txt" | tr -d ' ') baseline files predate rustfmt)"
else
  row "cargo fmt --check" FAIL "$(since $T)" "unformatted (run cargo fmt on them): $(echo $NEWDIRTY | tr '\n' ' ' | cut -c1-200)"; FAIL=1
fi

# ------------------------------------------------------------------ clippy
T=$(now)
cargo clippy --workspace --all-targets --message-format=short > "$OUT/clippy.log" 2>&1
CLIPPY_RC=$?
python3 - "$OUT/clippy.log" "$OUT/clippy.now" <<'EOF'
import re, sys, collections
seen = set(); cnt = collections.Counter()
for l in open(sys.argv[1], errors="replace"):
    m = re.match(r"^(\S+?):(\d+):(\d+): (warning|error)(?:\[[^\]]*\])?: (.*)$", l.rstrip())
    if not m: continue
    path, line, col, kind, msg = m.groups()
    if (path, line, col, msg) in seen: continue      # lib + lib test report the same warning twice
    seen.add((path, line, col, msg))
    cnt[(path, kind, re.sub(r"\s+", " ", msg)[:160])] += 1
with open(sys.argv[2], "w") as f:
    for (p, k, msg), n in sorted(cnt.items()): f.write(f"{n}\t{p}\t{k}\t{msg}\n")
EOF
if [ $UPDATE = 1 ]; then cp "$OUT/clippy.now" "$BASELINES/clippy.txt"; fi
[ -f "$BASELINES/clippy.txt" ] || : > "$BASELINES/clippy.txt"
CL=$(python3 - "$BASELINES/clippy.txt" "$OUT/clippy.now" <<'EOF'
import sys, collections
def load(p):
    d = {}
    for l in open(p):
        n, path, kind, msg = l.rstrip("\n").split("\t", 3); d[(path, kind, msg)] = int(n)
    return d
base, now = load(sys.argv[1]), load(sys.argv[2])
new = [(k, n - base.get(k, 0)) for k, n in now.items() if n > base.get(k, 0)]
total_b, total_n = sum(base.values()), sum(now.values())
if new:
    print("FAIL %d new (baseline %d, now %d): %s" % (sum(n for _, n in new), total_b, total_n, "; ".join(f"{k[0]}: {k[2][:60]}" for k, _ in new[:3])))
else:
    print("pass baseline %d warnings (%d now)" % (total_b, total_n))
EOF
)
if [ "${CL%% *}" = pass ]; then row "cargo clippy (new warnings)" pass "$(since $T)" "${CL#pass }"; else row "cargo clippy (new warnings)" FAIL "$(since $T)" "${CL#FAIL }"; FAIL=1; fi

# ------------------------------------------------------------------ unit tests
T=$(now)
if cargo test --workspace -q > "$OUT/cargo-test.log" 2>&1; then
  row "cargo test --workspace" pass "$(since $T)" "$(grep -E '^test result' "$OUT/cargo-test.log" | awk '{s+=$4} END {print s" tests passed"}')"
else
  row "cargo test --workspace" FAIL "$(since $T)" "$(grep -E 'FAILED|panicked|error' "$OUT/cargo-test.log" | head -3 | tr '\n' ' ' | cut -c1-200)"; FAIL=1
fi

# ------------------------------------------------------------------ harnesses
T=$(now)
if cargo build --release -p basalt-server -p basalt-check -q > "$OUT/build.log" 2>&1 && cargo build --release -p basalt-worldgen --example compare -q >> "$OUT/build.log" 2>&1; then
  ARGS=(); [ $VANILLA = 0 ] && ARGS+=(--no-vanilla)
  [ -n "$BASE" ] && ARGS+=(--base "$BASE")
  SEL=(--affected); [ $ALL = 1 ] && SEL=(--all)
  T=$(now)
  ./target/release/basalt-check "${SEL[@]}" ${ARGS[@]+"${ARGS[@]}"} > "$OUT/pr_ready-harness.log" 2>&1
  RC=$?
  SUMMARY=$(grep -E '^[0-9]+ harness' "$OUT/pr_ready-harness.log" | tail -1)
  [ -n "$SUMMARY" ] || SUMMARY=$(grep -E '^RESULT run|nothing affected|no harness' "$OUT/pr_ready-harness.log" | tail -1)
  if [ $RC = 0 ]; then row "harnesses (${SEL[0]#--})" pass "$(since $T)" "$SUMMARY"; else row "harnesses (${SEL[0]#--})" FAIL "$(since $T)" "$SUMMARY; see $OUT/pr_ready-harness.log"; FAIL=1; fi
  if [ $CLIENT = 1 ]; then
    T=$(now)
    CSEL=(--affected); [ $ALL = 1 ] && CSEL=()
    ./target/release/basalt-check --group client ${CSEL[@]+"${CSEL[@]}"} ${ARGS[@]+"${ARGS[@]}"} > "$OUT/pr_ready-client.log" 2>&1
    RC=$?
    if grep -q "no harnesses are registered for group" "$OUT/pr_ready-client.log"; then row "harnesses (client group)" skip "$(since $T)" "no client harness group registered yet (note: nothing run)"
    elif [ $RC = 0 ]; then row "harnesses (client group)" pass "$(since $T)" "$(grep -E '^[0-9]+ harness|nothing affected' "$OUT/pr_ready-client.log" | tail -1 | cut -c1-120)"
    else row "harnesses (client group)" FAIL "$(since $T)" "see $OUT/pr_ready-client.log"; FAIL=1; fi
  fi
else
  row "build (release server + check)" FAIL "$(since $T)" "$(tail -3 "$OUT/build.log" | tr '\n' ' ' | cut -c1-200)"; FAIL=1
fi

# ------------------------------------------------------------------ table
echo
printf '%-32s %-6s %8s  %s\n' check result time details
while IFS=$'\t' read -r name res secs det; do printf '%-32s %-6s %7ss  %s\n' "$name" "$res" "$secs" "$det"; done < "$ROWS"
echo
if [ $FAIL = 0 ]; then echo "pr_ready: PASS"; else echo "pr_ready: FAIL"; fi
exit $FAIL
