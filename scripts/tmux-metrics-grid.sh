#!/usr/bin/env bash
#
# A tmux window that is a small dashboard for one resource: its detail pane
# and five of its metrics, each charted alone in its own pane — every pane
# its own neboto, started there with `neboto open … --metric … --only`.
#
#   ┌──────────────────────┬──────────────────────┐
#   │ detail pane          │ CPU                  │
#   ├──────────────────────┼──────────────────────┤
#   │ Connections          │ Read + Write IOPS    │
#   ├──────────────────────┼──────────────────────┤
#   │ Read + Write latency │ Freeable memory      │
#   └──────────────────────┴──────────────────────┘
#
# (tmux's `tiled` layout: a different number of METRICS tiles to fit.)
#
# A metric that shares a chart with others brings them along (Read IOPS
# comes with Write IOPS). The chart panes refresh once a minute (`-w`).
#
# Usage:
#   ./scripts/tmux-metrics-grid.sh                    # emulator: an RDS instance, the five above
#   ./scripts/tmux-metrics-grid.sh rds catalog-pg-01
#   METRICS="invocations,duration,concurrency" ./scripts/tmux-metrics-grid.sh lambda my-fn
#
# Env:
#   METRICS      comma-separated row labels or CloudWatch names, one pane each
#                (default: cpu,connections,read iops,read latency,freeable memory)
#   RANGE        chart range: 1h 6h 24h 7d 30d (default 6h)
#   ENDPOINT     emulator URL (default $AWS_ENDPOINT_URL or http://localhost:4566);
#                ENDPOINT= (empty) uses your real AWS credentials instead
#   REGION       region (default $AWS_REGION or us-east-1)
#   NEBOTO       neboto binary (default target/release, then target/debug, then PATH)
#   SESSION      tmux session name when run outside tmux (default neboto-grid)
#   NO_ATTACH=1  always build a separate detached session, even from inside tmux

set -euo pipefail

ENDPOINT="${ENDPOINT-${AWS_ENDPOINT_URL:-http://localhost:4566}}"
REGION="${REGION:-${AWS_REGION:-us-east-1}}"
SESSION="${SESSION:-neboto-grid}"
RANGE="${RANGE:-6h}"
METRICS="${METRICS:-cpu,connections,read iops,read latency,freeable memory}"

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
if [ -z "${NEBOTO:-}" ]; then
  for b in "$ROOT/target/release/neboto" "$ROOT/target/debug/neboto" "$(command -v neboto || true)"; do
    if [ -n "$b" ] && [ -x "$b" ]; then NEBOTO="$b"; break; fi
  done
fi
[ -n "${NEBOTO:-}" ] || { echo "✗ No neboto binary — cargo build, or set NEBOTO" >&2; exit 1; }
"$NEBOTO" open --help 2>/dev/null | grep -q -- '--only' \
  || { echo "✗ $NEBOTO has no \`open --only\` — build this checkout (cargo build)" >&2; exit 1; }
command -v tmux >/dev/null || { echo "✗ tmux not found" >&2; exit 1; }

FLAGS=(-r "$REGION" --no-banner --no-update-check)
if [ -n "$ENDPOINT" ]; then
  # Dummy creds for the emulator — never the caller's real ones (neboto
  # injects its own test/test whenever an endpoint is set).
  export AWS_ACCESS_KEY_ID=test AWS_SECRET_ACCESS_KEY=test
  unset AWS_SESSION_TOKEN AWS_PROFILE
  FLAGS+=(--endpoint-url "$ENDPOINT")
fi

TARGET=("$@")
if [ ${#TARGET[@]} -eq 0 ]; then
  [ -n "$ENDPOINT" ] || { echo "✗ Give a resource: $0 <service> <id>" >&2; exit 1; }
  # An instance (not an Aurora cluster member) the emulator publishes
  # storage for, so every default pane has data.
  DB=$(aws --endpoint-url "$ENDPOINT" --region "$REGION" --no-cli-pager --output text \
    cloudwatch list-metrics --namespace AWS/RDS --metric-name FreeStorageSpace \
    --query 'Metrics[].Dimensions[?Name==`DBInstanceIdentifier`].Value[]' 2>/dev/null | tr '\t' '\n' | head -1)
  [ -n "$DB" ] || { echo "✗ No RDS instance with metrics at $ENDPOINT" >&2; exit 1; }
  TARGET=(rds "$DB")
fi

IFS=',' read -r -a WANT <<<"$METRICS"
[ ${#WANT[@]} -ge 1 ] || { echo "✗ METRICS is empty" >&2; exit 1; }

cmd() { printf '%q ' "$NEBOTO" "$@"; }
DETAIL=$(cmd "${FLAGS[@]}" open "${TARGET[@]}")
NAME="${TARGET[${#TARGET[@]}-1]}"
echo "▸ ${TARGET[*]}${ENDPOINT:+ on $ENDPOINT} — ${#WANT[@]} metrics over $RANGE"

if [ -n "${TMUX:-}" ] && [ -z "${NO_ATTACH:-}" ]; then
  FIRST=$(tmux new-window -P -F '#{pane_id}' -n "$NAME" "$DETAIL")
else
  tmux kill-session -t "$SESSION" 2>/dev/null || true
  FIRST=$(tmux new-session -d -P -F '#{pane_id}' -s "$SESSION" -n "$NAME" \
    -x "$(tput cols 2>/dev/null || echo 240)" -y "$(tput lines 2>/dev/null || echo 60)" "$DETAIL")
fi
WINDOW=$(tmux display-message -p -t "$FIRST" '#{window_id}')

# Split from the newest pane each time, so the grid reads in METRICS order.
LAST="$FIRST"
for m in "${WANT[@]}"; do
  m="$(echo "$m" | sed 's/^ *//; s/ *$//')"
  [ -n "$m" ] || continue
  LAST=$(tmux split-window -P -F '#{pane_id}' -t "$LAST" \
    "$(cmd "${FLAGS[@]}" -w open "${TARGET[@]}" --charts "$RANGE" --metric "$m" --only --full)")
  # Re-tile as we go: a pane split too small refuses the next split.
  tmux select-layout -t "$WINDOW" tiled >/dev/null
done
tmux select-pane -t "$FIRST"

if [ -z "${NO_ATTACH:-}" ] && [ -z "${TMUX:-}" ]; then exec tmux attach -t "$SESSION"; fi
