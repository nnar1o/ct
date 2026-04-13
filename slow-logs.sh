#!/usr/bin/env bash

set -u

SECONDS_TOTAL="${1:-30}"
BURST_SIZE="${2:-3}"
INTERVAL_MS="${3:-100}"

if ! [[ "$SECONDS_TOTAL" =~ ^[0-9]+$ ]] || ! [[ "$BURST_SIZE" =~ ^[0-9]+$ ]] || ! [[ "$INTERVAL_MS" =~ ^[0-9]+$ ]]; then
  echo "usage: ./slow-logs.sh [seconds] [burst_size] [interval_ms]" >&2
  exit 2
fi

if [ "$SECONDS_TOTAL" -le 0 ] || [ "$BURST_SIZE" -le 0 ] || [ "$INTERVAL_MS" -le 0 ]; then
  echo "seconds, burst_size and interval_ms must be > 0" >&2
  exit 2
fi

TOTAL_TICKS=$((SECONDS_TOTAL * 1000 / INTERVAL_MS))
if [ "$TOTAL_TICKS" -le 0 ]; then
  TOTAL_TICKS=1
fi
INTERVAL_SEC="$(awk -v ms="$INTERVAL_MS" 'BEGIN { printf "%.3f", ms / 1000 }')"

for ((tick=1; tick<=TOTAL_TICKS; tick++)); do
  ts="$(date -u +"%Y-%m-%dT%H:%M:%S.%3NZ")"
  echo "[$ts] tick=$tick begin"

  for ((i=1; i<=BURST_SIZE; i++)); do
    echo "[$ts] info tick=$tick item=$i message=processing"
  done

  if (( tick % 12 == 0 )); then
    echo "[$ts] warning tick=$tick message=temporary slowdown" >&2
  fi

  if (( tick % 25 == 0 )); then
    echo "[$ts] error tick=$tick message=simulated transient failure" >&2
  fi

  echo "[$ts] tick=$tick end"
  sleep "$INTERVAL_SEC"
done

echo "done: emitted logs for ${SECONDS_TOTAL}s with burst=${BURST_SIZE} interval_ms=${INTERVAL_MS}"
