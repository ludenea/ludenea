#!/usr/bin/env bash
# Reads the record of the keys the cells reported: for every run of the
# research CI that measured this spike, the key each cell reported, the
# processor of its host and the bytes it drew, then per cell how often the
# key moved from one run to the next and how often the bytes moved under an
# unchanged key.
#
# Usage: key-record.sh [event] [limit]
#   event  the runs to read: schedule (default), workflow_dispatch or all
#   limit  how many of the workflow's latest runs to look at (default 200)
#
# Needs gh, signed in with read access to the repository, and jq. Run from a
# checkout of the repository. A run's artifacts are kept ninety days: the
# record of eight weeks is to be read before its first runs expire.
set -euo pipefail

event="${1:-schedule}"
limit="${2:-200}"
spike="visual-reference-round-trip"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

filter=(--event "$event")
if [ "$event" = all ]; then
  filter=()
fi

rows="$work/rows.tsv"
: > "$rows"
# Oldest first, so that a change is counted from one run to the next.
gh run list --workflow research-ci.yml ${filter[@]+"${filter[@]}"} --limit "$limit" \
  --json databaseId,createdAt --jq 'sort_by(.createdAt) | .[] | "\(.databaseId) \(.createdAt)"' > "$work/runs.txt"
while read -r run created; do
  directory="$work/$run"
  # A run of another spike, or one whose artifacts expired, has no such artifact.
  if ! gh run download "$run" --name "$spike-all-cells" --dir "$directory" >/dev/null 2>&1; then
    continue
  fi
  for facts in "$directory"/*/facts.json; do
    [ -f "$facts" ] || continue
    jq -r --arg run "$run" --arg created "$created" \
      'select(.key_hash) | [$created, $run, .cell, .key_hash, .frame_bytes_hash, .host_processor] | @tsv' "$facts" >> "$rows"
  done
  rm -rf "$directory"
done < "$work/runs.txt"

echo "| Run started | Run | Cell | Key | Frame bytes | Processor |"
echo "| --- | --- | --- | --- | --- | --- |"
awk -F'\t' '{ printf "| %s | %s | %s | %s | %s | %s |\n", $1, $2, $3, $4, $5, $6 }' "$rows"
echo
echo "| Cell | Runs | Distinct keys | Key changes from one run to the next | Distinct pictures | Changes of bytes under an unchanged key |"
echo "| --- | --- | --- | --- | --- | --- |"
sort -t "$(printf '\t')" -k3,3 -k1,1 "$rows" | awk -F'\t' '
  function flush() {
    if (cell != "") printf "| %s | %d | %d | %d | %d | %d |\n", cell, runs, keys, key_changes, pictures, byte_changes
  }
  $3 != cell {
    flush()
    cell = $3; runs = 0; keys = 0; key_changes = 0; pictures = 0; byte_changes = 0; previous_key = ""; previous_bytes = ""
    split("", seen_key); split("", seen_bytes)
  }
  {
    runs++
    if (!($4 in seen_key)) { seen_key[$4] = 1; keys++ }
    if (!($5 in seen_bytes)) { seen_bytes[$5] = 1; pictures++ }
    if (previous_key != "" && $4 != previous_key) key_changes++
    if (previous_key != "" && $4 == previous_key && $5 != previous_bytes) byte_changes++
    previous_key = $4; previous_bytes = $5
  }
  END { flush() }'
