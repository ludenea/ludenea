#!/usr/bin/env bash
# Prints a Markdown summary of one spike's measurements across cells.
#
# Usage: ci/summarize.sh <spike directory> <out directory>
#   <out directory>  holds one <cell>/facts.json per cell, as run-spike.sh writes them
#
# The research CI appends the output to the run summary. Needs jq. Portable
# shell on purpose: it runs the same on a developer machine.
set -euo pipefail

spike="${1:?usage: summarize.sh <spike directory> <out directory>}"
dir="${2:?usage: summarize.sh <spike directory> <out directory>}"

shopt -s nullglob
facts=("$dir"/*/facts.json)

echo "## $spike"
echo
echo "${#facts[@]} cell(s) reported."
echo
if [ "${#facts[@]}" -eq 0 ]; then
  exit 0
fi

case "$spike" in
  platform-facts)
    echo "| Cell | exact | codegen | libm | nan | ps per iteration (median) |"
    echo "| --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '"| \(.cell) | \(.hashes.exact) | \(.hashes.codegen) | \(.hashes.libm) | \(.hashes.nan) | \(.timing.picoseconds_per_iteration_median) |"' "$file"
    done
    echo
    for group in exact codegen libm nan; do
      distinct="$(jq -r ".hashes.$group" "${facts[@]}" | sort -u | wc -l | tr -d ' ')"
      echo "- \`$group\`: $distinct distinct hash value(s) across ${#facts[@]} cell(s)."
    done
    echo
    echo "Platform math library on the alternate dispatch path of the same machine (x86_64 cells only):"
    echo
    for file in "${facts[@]}"; do
      jq -r '"- `\(.cell)`: \(.alternate_dispatch.method // "not measured"); libm hash \(.alternate_dispatch.libm // "n/a"); probes that differ from the default path: \(.alternate_dispatch.differs_from_default // [] | if length == 0 then "none" else join(", ") end)."' "$file"
    done
    ;;
  box2d-worker-sweep)
    echo "| Cell | SIMD width | Scheduler | Scene | Workers | Final hash | Steps | ms per step (median) | First divergent step |"
    echo "| --- | --- | --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .runs[] | "| \($cell) | \(.simd_width) | \(.scheduler) | \(.scene) | \(.workers) | \(.final_hash) | \(.steps) | \(.step_ms_median) | \(.first_divergent_step) |"' "$file"
    done
    echo
    echo "Distinct final hashes per scene across every cell, SIMD width, scheduler and worker count (1 means identical everywhere):"
    echo
    for scene in $(jq -r '.runs[].scene' "${facts[@]}" | sort -u); do
      distinct="$(jq -r --arg scene "$scene" '.runs[] | select(.scene == $scene) | .final_hash' "${facts[@]}" | sort -u | wc -l | tr -d ' ')"
      echo "- \`$scene\`: $distinct distinct final hash value(s)."
    done
    echo
    echo "| Cell | Falling hinges hash | Sleep step | Matches the upstream expectation |"
    echo "| --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .falling_hinges[] | "| \($cell) (\(.scheduler), \(.workers) workers, width \(.simd_width)) | \(.hash) | \(.sleep_step) | \(.matches_upstream_expectation) |"' "$file"
    done
    ;;
  box2d-snapshot-continuation)
    echo "| Cell | Scene | Workers | Bodies hashed | Snapshot bytes | Snapshot ms | Path | Path workers | Restore succeeded | Hash at restore matches | First divergent step | Restore ms |"
    echo "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .runs[] | . as $run | .paths[] | "| \($cell) | \($run.scene) | \($run.workers) | \($run.hashed_body_count) of \($run.body_count) | \($run.snapshot_bytes) | \($run.snapshot_ms) | \(.path) | \(.workers) | \(.restore_succeeded) | \(.hash_at_restore_matches) | \(.first_divergent_step) | \(.restore_ms) |"' "$file"
    done
    echo
    echo "Paths that reproduce the uninterrupted run at every step after the snapshot, per cell:"
    echo
    for file in "${facts[@]}"; do
      jq -r '"- `\(.cell)`: \([.runs[].paths[] | select(.restore_succeeded and .hash_at_restore_matches and .first_divergent_step == -1)] | length) of \([.runs[].paths[]] | length) paths."' "$file"
    done
    echo
    echo "Distinct uninterrupted final hashes per scene and worker count across every cell (1 means identical everywhere):"
    echo
    for config in $(jq -r '.runs[] | "\(.scene)/\(.workers)"' "${facts[@]}" | sort -u); do
      distinct="$(jq -r --arg config "$config" '.runs[] | select("\(.scene)/\(.workers)" == $config) | .uninterrupted_final_hash' "${facts[@]}" | sort -u | wc -l | tr -d ' ')"
      echo "- \`$config\`: $distinct distinct final hash value(s)."
    done
    ;;
  reference-workload)
    echo "| Cell | Debug assertions | Size | Ticks | Wander | Run hash | Final hash | Total ms | Ticks per second |"
    echo "| --- | --- | --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .build.debug_assertions as $debug | .runs[] | "| \($cell) | \($debug) | \(.size) | \(.tick_count) | \(.wander) | \(.run_hash) | \(.final_hash) | \(.total_ms) | \(.ticks_per_second) |"' "$file"
    done
    echo
    echo "Distinct run hashes per configuration across every cell (1 means identical everywhere):"
    echo
    for config in $(jq -r '.runs[] | "\(.size)/\(.tick_count)/wander=\(.wander)"' "${facts[@]}" | sort -u); do
      distinct="$(jq -r --arg config "$config" '.runs[] | select("\(.size)/\(.tick_count)/wander=\(.wander)" == $config) | .run_hash' "${facts[@]}" | sort -u | wc -l | tr -d ' ')"
      echo "- \`$config\`: $distinct distinct run hash value(s)."
    done
    ;;
  workload-worker-sweep)
    echo "| Cell | Mode | Workers | Wander | Run hash | Matches the cell's sequential run | First divergent tick | Total ms | Ticks per second |"
    echo "| --- | --- | --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .runs[] | "| \($cell) | \(.mode) | \(.workers) | \(.wander) | \(.run_hash) | \(.matches_sequential) | \(.first_divergent_tick) | \(.total_ms) | \(.ticks_per_second) |"' "$file"
    done
    echo
    echo "Distinct run hashes across every cell and worker count, per mode and wander setting (1 means identical everywhere):"
    echo
    for config in $(jq -r '.runs[] | "\(.mode)/wander=\(.wander)"' "${facts[@]}" | sort -u); do
      distinct="$(jq -r --arg config "$config" '.runs[] | select("\(.mode)/wander=\(.wander)" == $config) | .run_hash' "${facts[@]}" | sort -u | wc -l | tr -d ' ')"
      echo "- \`$config\`: $distinct distinct run hash value(s)."
    done
    echo
    echo "Per cell: debug assertions, available CPUs, and the deterministic runs that match the sequential run of the same cell:"
    echo
    for file in "${facts[@]}"; do
      jq -r '"- `\(.cell)`: debug assertions \(.build.debug_assertions), \(.platform.available_parallelism) CPUs, \([.runs[] | select(.mode == "deterministic" and .matches_sequential)] | length) of \([.runs[] | select(.mode == "deterministic")] | length) deterministic runs match, \([.runs[] | select(.mode == "free-running" and .matches_sequential)] | length) of \([.runs[] | select(.mode == "free-running")] | length) free-running runs match."' "$file"
    done
    ;;
  *)
    echo "No summary is defined for this spike; the raw facts are in the artifacts."
    ;;
esac
