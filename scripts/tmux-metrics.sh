#!/usr/bin/env bash
#
# A tmux window on one resource: its detail pane on the left, its charts at
# two ranges on the right — each pane its own neboto, started there with
# `neboto open`.
#
#   ┌──────────────────┬──────────────────┐
#   │                  │ charts · 1h      │
#   │  detail pane     │                  │
#   │  (Overview)      ├──────────────────┤
#   │                  │ charts · 7d      │
#   └──────────────────┴──────────────────┘
#
# The chart panes run with `--watch` (they refresh once a minute) and
# `--full` (the charts take the pane's width).
#
# Usage:
#   ./scripts/tmux-metrics.sh                      # emulator: first EC2 instance with metrics
#   ./scripts/tmux-metrics.sh ec2 i-0abc123…       # any resource with charts
#   ./scripts/tmux-metrics.sh rds prod-main
#   ./scripts/tmux-metrics.sh arn:aws:lambda:…     # an ARN on its own
#
# Env:
#   ENDPOINT     emulator URL (default $AWS_ENDPOINT_URL or http://localhost:4566);
#                ENDPOINT= (empty) uses your real AWS credentials instead
#   REGION       region (default $AWS_REGION or us-east-1)
#   NEBOTO       neboto binary (default target/release, then target/debug, then PATH)
#   SESSION      tmux session name when run outside tmux (default neboto-metrics)
#   TOP / BOTTOM chart ranges: 1h 6h 24h 7d 30d (default 1h / 7d)
#   TOP_METRIC / BOTTOM_METRIC  the metric each chart pane opens on, by row
#                label or CloudWatch name (default: the first row), e.g.
#                TOP=24h TOP_METRIC=cpu BOTTOM=24h BOTTOM_METRIC="network in".
#                A pane with a metric charts only it (`--only`); ONLY=0 keeps
#                the other rows, with the cursor on it
#   NO_ATTACH=1  always build a separate detached session, even from inside tmux

set -euo pipefail

ENDPOINT="${ENDPOINT-${AWS_ENDPOINT_URL:-http://localhost:4566}}"
REGION="${REGION:-${AWS_REGION:-us-east-1}}"
SESSION="${SESSION:-neboto-metrics}"
TOP="${TOP:-1h}"
BOTTOM="${BOTTOM:-7d}"

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
if [ -z "${NEBOTO:-}" ]; then
  for b in "$ROOT/target/release/neboto" "$ROOT/target/debug/neboto" "$(command -v neboto || true)"; do
    if [ -n "$b" ] && [ -x "$b" ]; then NEBOTO="$b"; break; fi
  done
fi
[ -n "${NEBOTO:-}" ] || { echo "✗ No neboto binary — cargo build, or set NEBOTO" >&2; exit 1; }
"$NEBOTO" open --help >/dev/null 2>&1 \
  || { echo "✗ $NEBOTO has no \`open\` — build this checkout (cargo build)" >&2; exit 1; }
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
  # The first instance the emulator publishes CPU for, so the charts aren't empty.
  INSTANCE=$(aws --endpoint-url "$ENDPOINT" --region "$REGION" --no-cli-pager --output text \
    cloudwatch list-metrics --namespace AWS/EC2 --metric-name CPUUtilization \
    --query 'Metrics[].Dimensions[?Name==`InstanceId`].Value[]' 2>/dev/null | tr '\t' '\n' | head -1)
  [ -n "$INSTANCE" ] || { echo "✗ No EC2 instance with metrics at $ENDPOINT" >&2; exit 1; }
  TARGET=(ec2 "$INSTANCE")
fi

# One shell-quoted command line per pane. TUI flags (-w) go before `open`.
cmd() { printf '%q ' "$NEBOTO" "$@"; }
LEFT=$(cmd "${FLAGS[@]}" open "${TARGET[@]}")
charts() { # $1 range, $2 metric (optional)
  local only=(); [ -n "${2:-}" ] && [ "${ONLY:-1}" = "1" ] && only=(--only)
  cmd "${FLAGS[@]}" -w open "${TARGET[@]}" --charts "$1" ${2:+--metric "$2"} ${only[@]+"${only[@]}"} --full
}
RIGHT_TOP=$(charts "$TOP" "${TOP_METRIC:-}")
RIGHT_BOTTOM=$(charts "$BOTTOM" "${BOTTOM_METRIC:-}")
NAME="${TARGET[${#TARGET[@]}-1]}"
echo "▸ ${TARGET[*]}${ENDPOINT:+ on $ENDPOINT}"

if [ -n "${TMUX:-}" ] && [ -z "${NO_ATTACH:-}" ]; then
  LEFT_PANE=$(tmux new-window -P -F '#{pane_id}' -n "$NAME" "$LEFT")
else
  tmux kill-session -t "$SESSION" 2>/dev/null || true
  LEFT_PANE=$(tmux new-session -d -P -F '#{pane_id}' -s "$SESSION" -n "$NAME" \
    -x "$(tput cols 2>/dev/null || echo 200)" -y "$(tput lines 2>/dev/null || echo 50)" "$LEFT")
fi
TOP_PANE=$(tmux split-window -P -F '#{pane_id}' -h -l 55% -t "$LEFT_PANE" "$RIGHT_TOP")
tmux split-window -v -l 50% -t "$TOP_PANE" "$RIGHT_BOTTOM"
tmux select-pane -t "$LEFT_PANE"

if [ -z "${NO_ATTACH:-}" ] && [ -z "${TMUX:-}" ]; then exec tmux attach -t "$SESSION"; fi
