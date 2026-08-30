#!/usr/bin/env bash
# Poll /api/v1/links into timestamped JSONL.
#
# One line per poll: {"t": <unix seconds>, "links": [...]} — the endpoint's own
# payload, unmodified, so nothing is lost to a summarising step I would have to
# re-run if the analysis changes.
#
# Used for two things: an empty-room calibration baseline (per-link resting
# scale and its distribution), and a labelled walk. The difference is only which
# segment of the file you look at, so the recorder itself stays dumb.
#
# Usage: record_links.sh <output.jsonl> [poll_seconds]
set -u

OUT="${1:?usage: record_links.sh <output.jsonl> [poll_seconds]}"
PERIOD="${2:-1}"
URL="http://127.0.0.1:3000/api/v1/links"

mkdir -p "$(dirname "$OUT")"
echo "recording $URL every ${PERIOD}s -> $OUT" >&2

while true; do
  # `date +%s.%N` and the payload are joined here rather than server-side so a
  # dropped or slow poll is visible as a gap in t, not silently interpolated.
  TS=$(date +%s)
  BODY=$(curl -s -m 3 "$URL")
  if [ -n "$BODY" ]; then
    printf '{"t":%s,"payload":%s}\n' "$TS" "$BODY" >> "$OUT"
  else
    printf '{"t":%s,"error":"poll failed"}\n' "$TS" >> "$OUT"
  fi
  sleep "$PERIOD"
done
