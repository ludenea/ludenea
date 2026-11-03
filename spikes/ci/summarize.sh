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
  platform-threading-constraints)
    echo "| Cell | Probe | Outcome | Exit | Duration ms | Details |"
    echo "| --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .probes[] | "| \($cell) | \(.name) | \(.outcome) | \(.exit) | \(.duration_ms) | \(.details | to_entries | map("\(.key)=\(.value)") | join("; ") | gsub("\\|"; "\\\\|")) |"' "$file"
    done
    echo
    echo "Session facts per cell:"
    echo
    for file in "${facts[@]}"; do
      jq -r '"- `\(.cell)`: \(.session | to_entries | map("\(.key)=\(.value)") | join("; "))"' "$file"
    done
    echo
    echo "Standard error tails of the probes that did not succeed:"
    echo
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .probes[] | select(.outcome != "succeeded") | "- `\($cell)` `\(.name)` (\(.outcome)): \(.stderr_tail | gsub("\n"; " ") | .[0:300])"' "$file"
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
  workload-scheduling-overhead)
    echo "| Cell | Scheduler | Waiting | Spin bound (us) | Chunk | Workers | Run hash | Matches the cell's sequential run | First divergent tick | Total ms | Ticks per second | Utilization |"
    echo "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .runs[] | "| \($cell) | \(.scheduler) | \(.waiting) | \(.spin_bound_us) | \(.chunk_size) | \(.workers) | \(.run_hash) | \(.matches_sequential) | \(.first_divergent_tick) | \(.total_ms) | \(.ticks_per_second) | \(.utilization // "n/a") |"' "$file"
    done
    echo
    distinct="$(jq -r '.runs[].run_hash, .chunk_size_sweep[].run_hash, .spin_bound_sweep[].run_hash' "${facts[@]}" | sort -u | wc -l | tr -d ' ')"
    echo "- Distinct run hashes across every cell, scheduler, waiting policy, chunk size and worker count (1 means identical everywhere): $distinct."
    echo
    echo "Per cell: debug assertions, available CPUs, and the parallel runs that match the sequential run of the same cell:"
    echo
    for file in "${facts[@]}"; do
      jq -r '"- `\(.cell)`: debug assertions \(.build.debug_assertions), \(.platform.available_parallelism) CPUs, \([.runs[], .chunk_size_sweep[], .spin_bound_sweep[] | select(.scheduler != "sequential" and .matches_sequential)] | length) of \([.runs[], .chunk_size_sweep[], .spin_bound_sweep[] | select(.scheduler != "sequential")] | length) parallel runs match; panic injection: pool terminated \(.panic_injection.pool_terminated), failure reported \(.panic_injection.failure_reported) at tick \(.panic_injection.failure.tick // "n/a") in \(.panic_injection.failure.node // "n/a") chunk \(.panic_injection.failure.chunk // "n/a"), \(.panic_injection.ticks_hashed) ticks hashed, hashes before the failure match \(.panic_injection.hashes_before_the_failure_match)."' "$file"
    done
    echo
    echo "Chunk size sweep (ticks per second):"
    echo
    echo "| Cell | Scheduler | Chunk | Workers | Matches | Ticks per second | Utilization |"
    echo "| --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .chunk_size_sweep[] | "| \($cell) | \(.scheduler) | \(.chunk_size) | \(.workers) | \(.matches_sequential) | \(.ticks_per_second) | \(.utilization) |"' "$file"
    done
    echo
    echo "Spin bound sweep of the counter graph (ticks per second):"
    echo
    echo "| Cell | Spin bound (us) | Workers | Matches | Ticks per second | Utilization | Parks |"
    echo "| --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .spin_bound_sweep[] | "| \($cell) | \(.spin_bound_us) | \(.workers) | \(.matches_sequential) | \(.ticks_per_second) | \(.utilization) | \(.parks) |"' "$file"
    done
    echo
    echo "Empty kernel: the scheduler alone, per tick and per task:"
    echo
    echo "| Cell | Scheduler | Workers | Ticks | Tasks per tick | Microseconds per tick | Microseconds per task | Utilization | Parks | Steals |"
    echo "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .empty_graph[] | "| \($cell) | \(.scheduler) | \(.workers) | \(.ticks) | \(.tasks_per_tick) | \(.microseconds_per_tick) | \(.microseconds_per_task) | \(.utilization) | \(.parks) | \(.steals) |"' "$file"
    done
    ;;
  workload-storage-layouts)
    echo "| Cell | Layout | Mode | Change detection | Workers | Run hash | Matches the cell's sequential run | First divergent tick | Heat hash | Total ms | Ticks per second |"
    echo "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .runs[] | "| \($cell) | \(.layout) | \(.mode) | \(.change_detection) | \(.workers) | \(.run_hash) | \(.matches_sequential) | \(.first_divergent_tick) | \(.heat_hash) | \(.total_ms) | \(.ticks_per_second) |"' "$file"
    done
    echo
    distinct="$(jq -r '.runs[].run_hash' "${facts[@]}" | sort -u | wc -l | tr -d ' ')"
    echo "- Distinct run hashes across every cell, layout, mode, change detection and worker count (1 means identical everywhere): $distinct."
    heat="$(jq -r '.runs[] | select(.mode == "two-archetypes") | .heat_hash' "${facts[@]}" | sort -u | wc -l | tr -d ' ')"
    echo "- Distinct heat hashes across every cell, layout and worker count in the two-archetype mode (1 means identical everywhere): $heat."
    echo
    echo "Per cell: debug assertions, available CPUs, and the parallel runs that match the sequential run of the same cell:"
    echo
    for file in "${facts[@]}"; do
      jq -r '"- `\(.cell)`: debug assertions \(.build.debug_assertions), \(.platform.available_parallelism) CPUs, \([.runs[] | select(.layout != "sequential" and .matches_sequential)] | length) of \([.runs[] | select(.layout != "sequential")] | length) parallel runs match."' "$file"
    done
    echo
    echo "Main-thread wall time per phase, milliseconds per 1 000 ticks:"
    echo
    echo "| Cell | Layout | Mode | Change detection | Workers | Canonical traversal | Spatial hashing | Repulsion | Motion | Cooling | Structural changes | Hash |"
    echo "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .runs[] | select(.layout != "sequential") | "| \($cell) | \(.layout) | \(.mode) | \(.change_detection) | \(.workers) | \(.phase_ms.canonical_traversal) | \(.phase_ms.spatial_hashing) | \(.phase_ms.repulsion) | \(.phase_ms.motion) | \(.phase_ms.cooling) | \(.phase_ms.structural_changes) | \(.phase_ms.hash) |"' "$file"
    done
    ;;
  workload-structural-changes)
    echo "| Cell | Layout | Recording | Allocation | Policy | Rate | Workers | Follows the specification | Run hash | Matches the cell's sequential run | Heat hash | Ticks per second | Changes | Microseconds per change | Apply share of the tick | Overflow spawns | Returned reservations | Final slots |"
    echo "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .runs[] | "| \($cell) | \(.layout) | \(.recording) | \(.allocation) | \(.policy) | \(.rate) | \(.workers) | \(.follows_the_specification) | \(.run_hash) | \(.matches_sequential) | \(.heat_hash) | \(.ticks_per_second) | \(.changes.total) | \(.microseconds_per_change) | \(.apply_share_of_tick) | \(.changes.overflow_spawns) | \(.changes.returned_reservations) | \(.final_slot_count) |"' "$file"
    done
    echo
    echo "Distinct run hashes per configuration across every cell and worker count (1 means identical everywhere):"
    echo
    for config in $(jq -r '.runs[] | "\(.layout)/\(.recording)/\(.allocation)/\(.policy)/\(.rate)"' "${facts[@]}" | sort -u); do
      distinct="$(jq -r --arg config "$config" '.runs[] | select("\(.layout)/\(.recording)/\(.allocation)/\(.policy)/\(.rate)" == $config) | .run_hash' "${facts[@]}" | sort -u | wc -l | tr -d ' ')"
      heat="$(jq -r --arg config "$config" '.runs[] | select("\(.layout)/\(.recording)/\(.allocation)/\(.policy)/\(.rate)" == $config) | .heat_hash' "${facts[@]}" | sort -u | wc -l | tr -d ' ')"
      echo "- \`$config\`: $distinct distinct run hash value(s), $heat distinct heat hash value(s)."
    done
    echo
    echo "Per cell: the runs that follow the specification and match the sequential run of the same cell:"
    echo
    for file in "${facts[@]}"; do
      jq -r '"- `\(.cell)`: debug assertions \(.build.debug_assertions), \(.platform.available_parallelism) CPUs, \([.runs[] | select(.follows_the_specification and .matches_sequential)] | length) of \([.runs[] | select(.follows_the_specification)] | length) specified runs match, \([.runs[] | select((.follows_the_specification | not) and .matches_sequential)] | length) unspecified runs match (expected 0)."' "$file"
    done
    ;;
  runtime-skeleton)
    echo "Time control scenarios per configuration and worker count (every world compared with the sequential reference of its seed):"
    echo
    echo "| Cell | Configuration | Workers | Headless matches | Ticks per second | Extracted ticks | Harness batches match | Snapshot restored matches | Round trip identical | Final snapshots identical | Isolation (first, second) | Failure reported at tick | Failed world last good tick | Failed world matches | Step after failure rejected | Healthy world matches |"
    echo "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | (.time_control // {}) | to_entries[] | .key as $configuration | .value.runs[]? | "| \($cell) | \($configuration) | \(.workers) | \(.headless.matches_reference) | \(.headless.ticks_per_second) | \(.headless.extracted_ticks) | \([.harness[] | .matches_reference] | all) | \(.snapshot.restored.matches_reference) | \(.snapshot.snapshot_round_trip_identical) | \(.snapshot.final_snapshots_identical) | \(.isolation.first_world.matches_reference), \(.isolation.second_world.matches_reference) | \(.failure.reported.tick // "none") | \(.failure.failed_world.tick) | \(.failure.failed_world.matches_reference) | \(.failure.failed_world.step_after_failure_rejected) | \(.failure.healthy_world.matches_reference) |"' "$file"
    done
    echo
    echo "Harness host, ticks per second while stepping, per batch size (pause of 1 ms after every batch unless stated):"
    echo
    echo "| Cell | Configuration | Workers | Batch | Pause | Alternating threads | Matches | Ticks per second while stepping | Paused ms |"
    echo "| --- | --- | --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | (.time_control // {}) | to_entries[] | .key as $configuration | .value.runs[]? | .workers as $workers | .harness[] | "| \($cell) | \($configuration) | \($workers) | \(.batch) | \(.pause_after_every_batch) | \(.alternating_threads) | \(.matches_reference) | \(.ticks_per_second_while_stepping) | \(.paused_ms) |"' "$file"
    done
    echo
    echo "Crate graph of the host per configuration:"
    echo
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | (.build_measurements.crate_graph // {}) | to_entries[] | "- `\($cell)` \(.key): \(.value.crates) crates, presentation crate in the graph: \(.value.presentation_crate_in_graph) (\([.value.names[].name] | join(", ")))."' "$file"
    done
    echo
    echo "Build times in seconds (clean builds from an empty target directory; incremental builds after a one-line change in the core):"
    echo
    echo "| Cell | Configuration | Profile | Kind | Sample | Seconds | Succeeded | Presentation library built |"
    echo "| --- | --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | (.build_measurements.builds // [])[] | "| \($cell) | \(.configuration) | \(.profile) | \(.kind) | \(.sample) | \(.seconds) | \(.succeeded) | \(.presentation_library_built // "n/a") |"' "$file"
    done
    echo
    echo "Headless facts per cell:"
    echo
    for file in "${facts[@]}"; do
      jq -r '"- `\(.cell)`: built configuration \(.built_configuration), \(.platform.available_parallelism) CPUs, DISPLAY set \(.headless.display_set), WAYLAND_DISPLAY set \(.headless.wayland_display_set); \(.headless.platform_libraries)."' "$file"
    done
    ;;
  workload-frame-allocator)
    echo "| Cell | Mode | Workers | Run hash | Matches the cell's sequential run | First divergent tick | Event hash | Ticks per second | Steady-state heap allocations per tick (mean, max) | Steady-state heap bytes per tick (mean) | First ten ticks allocations | Last tick with an allocation | Heap net blocks after destruction | Scratch high water (bytes) | Scratch fallback blocks | Leak free |"
    echo "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .runs[] | "| \($cell) | \(.mode) | \(.workers) | \(.run_hash) | \(.matches_sequential) | \(.first_divergent_tick) | \(.event_hash // "n/a") | \(.ticks_per_second) | \(.heap.steady_state_allocations_per_tick_mean // "n/a"), \(.heap.steady_state_allocations_per_tick_max // "n/a") | \(.heap.steady_state_bytes_per_tick_mean // "n/a") | \(.heap.first_ten_ticks_allocations // "n/a") | \(.heap.last_tick_with_allocation // "n/a") | \(.heap.net_blocks_after_destruction // "not counted") | \(.scratch.high_water_bytes // "n/a") | \(.scratch.fallback_blocks // "n/a") | \(.leak_free // "n/a") |"' "$file"
    done
    echo
    distinct="$(jq -r '.runs[].run_hash' "${facts[@]}" | sort -u | wc -l | tr -d ' ')"
    echo "- Distinct run hashes across every cell, mode and worker count (1 means identical everywhere): $distinct."
    events="$(jq -r '.runs[] | select(.mode != "sequential") | .event_hash' "${facts[@]}" | sort -u | wc -l | tr -d ' ')"
    echo "- Distinct event hashes across every cell, mode and worker count (1 means identical everywhere): $events."
    echo
    echo "Growth runs (the grow variant against the preallocated one; hashes compare across variants and worker counts, not with the reference):"
    echo
    echo "| Cell | Variant | Rate | Workers | Ticks | Run hash | Event hash | Ticks per second | Final population | Final capacity | Table bytes | Growth events | Growth ms | Growth share of the run | Steady-state heap allocations per tick (mean) | Leak free |"
    echo "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .growth_runs[] | "| \($cell) | \(.variant) | \(.rate) | \(.workers) | \(.tick_count) | \(.run_hash) | \(.event_hash) | \(.ticks_per_second) | \(.tables.final_population) | \(.tables.final_capacity) | \(.tables.bytes) | \(.tables.growth_events) | \(.tables.growth_ms) | \(.tables.growth_share_of_run) | \(.heap.steady_state_allocations_per_tick_mean) | \(.leak_free) |"' "$file"
    done
    echo
    for config in $(jq -r '.growth_runs[] | "\(.rate)"' "${facts[@]}" | sort -u); do
      distinct="$(jq -r --arg rate "$config" '.growth_runs[] | select((.rate | tostring) == $rate) | "\(.run_hash)/\(.event_hash)"' "${facts[@]}" | sort -u | wc -l | tr -d ' ')"
      echo "- Rate $config: $distinct distinct (run hash, event hash) pair(s) across every cell, variant and worker count (1 means growth changed nothing)."
    done
    echo
    echo "Leak run (one pre-sized event buffer forgotten on purpose) and the tags at destruction:"
    echo
    for file in "${facts[@]}"; do
      jq -r '"- `\(.cell)`: leak free \(.leak_run.leak_free); tags: \(.leak_run.tags_at_destruction | to_entries | map("\(.key) \(.value.bytes) bytes") | join(", ")); heap net blocks \(.leak_run.heap.net_blocks_after_destruction // "not counted"), bytes \(.leak_run.heap.net_bytes_after_destruction // "not counted")."' "$file"
    done
    echo
    echo "Allocator microbenchmark, nanoseconds per allocation and free pair of 1 KB (median of three rounds, minimum):"
    echo
    echo "| Cell | Path | Threads | Median ns | Min ns |"
    echo "| --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .allocator_microbenchmark[] | "| \($cell) | \(.path) | \(.threads) | \(.nanoseconds_per_pair_median) | \(.nanoseconds_per_pair_min) |"' "$file"
    done
    ;;
  workload-schema)
    echo "| Cell | Server schema | Client script | Ticks read | Client hashes match the storage | First mismatching tick | Client matches the reference | Handles inconsistent at | Spin matches | State bytes | Description bytes | Encode us | Client read us | Storage hash us | Tick us |"
    echo "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .readings[] | "| \($cell) | \(.server_schema_version) | \(.client_script) | \(.ticks_read) | \(.client_hashes_match_storage) | \(.first_mismatching_tick) | \(.client_matches_reference) | \(.inconsistent_ticks) | \(.spin_matches_storage // "not read") | \(.state_bytes) | \(.description_bytes) | \(.encode_us_per_read) | \(.client_read_us_per_read) | \(.storage_hash_us_per_read) | \(.tick_us) |"' "$file"
    done
    echo
    echo "A state saved under schema version 1 at the save tick and loaded under version 2, then read by both scripts:"
    echo
    echo "| Cell | Save tick | Loaded | Hash at load matches the save | Spin is the default at load | Script | Client hashes match the storage | Client matches the reference | Spin matches |"
    echo "| --- | --- | --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .saved_under_v1_loaded_under_v2 as $l | $l.by_script[]? | "| \($cell) | \($l.save_tick) | \($l.loaded) | \($l.hash_at_load_matches_saved) | \($l.spin_is_the_default_at_load) | \(.client_script) | \(.client_hashes_match_storage) | \(.client_matches_reference) | \(.spin_matches_storage // "not read") |"' "$file"
    done
    echo
    echo "The breaking direction (the new script against the old description), per cell:"
    echo
    for file in "${facts[@]}"; do
      jq -r '"- `\(.cell)`: rejected \(.new_script_against_old_description.rejected): \(.new_script_against_old_description.error // "")."' "$file"
    done
    ;;
  control-surface-toy)
    echo "Paths per worker count (the agent's announced hashes against the sequential oracle; the input log replayed on a fresh world):"
    echo
    echo "| Cell | Workers | Path | Transport | Matches the oracle | First divergent tick | Ticks per second | Direct ticks per second | Step us per call | Events | Contiguous | Duplicates | Step hashes match events | Applied | Stale | Reconnected | Retried key alike | Bytes sent | Bytes received | Log bytes per tick | Replay matches |"
    echo "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .runs[] | .workers as $w | .direct.ticks_per_second as $direct | .paths[] | "| \($cell) | \($w) | \(.path) | \(.transport) | \(.matches_oracle) | \(.first_divergent_tick) | \(.ticks_per_second) | \($direct) | \(.step_us_per_call) | \(.events_received) | \(.events_contiguous) | \(.duplicate_events) | \(.step_hashes_match_events) | \(.outcomes_applied) | \(.outcomes_stale) | \(.reconnected) | \(.retried_key_answered_alike) | \(.bytes_sent) | \(.bytes_received) | \(.log_bytes_per_tick) | \(.replay_matches) |"' "$file"
    done
    echo
    echo "Per cell: the direct run against the oracle, and the paths that match it:"
    echo
    for file in "${facts[@]}"; do
      jq -r '"- `\(.cell)`: \(.platform.available_parallelism) CPUs; direct matches the oracle at every worker count: \([.runs[].direct.matches_oracle] | all); \([.runs[].paths[] | select(.matches_oracle and .replay_matches)] | length) of \([.runs[].paths[]] | length) paths match the oracle and replay."' "$file"
    done
    echo
    echo "Round trips (one-tick steps with a query, and empty polls):"
    echo
    echo "| Cell | Workers | Transport | Step us per call | World step us per call | Surface and transport us per step | Empty round trip us | Matches the oracle |"
    echo "| --- | --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .runs[] | .workers as $w | .round_trips[] | "| \($cell) | \($w) | \(.transport) | \(.step_us_per_call) | \(.world_step_us_per_call) | \(.surface_and_transport_us_per_step) | \(.empty_round_trip_us) | \(.matches_oracle) |"' "$file"
    done
    echo
    echo "The paced session over the loopback socket (the host steps; the agent polls, kicks, disconnects once and resumes):"
    echo
    echo "| Cell | Workers | Host steps | Host ticks per second | Events | Contiguous | Duplicates | Every tick announced once | Announced hashes match the world | Commands | Applied | Reconnected | Replay matches |"
    echo "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .paced | "| \($cell) | \(.workers) | \(.host_steps) | \(.host_ticks_per_second) | \(.events_received) | \(.events_contiguous) | \(.duplicate_events) | \(.every_tick_announced_once) | \(.announced_hashes_match_the_world) | \(.commands_sent) | \(.outcomes_applied) | \(.reconnected) | \(.replay_matches) |"' "$file"
    done
    echo
    echo "The compatible change (a field added to the kick command with a default):"
    echo
    echo "| Cell | Run | Transport | Matches its oracle | Replay matches | Commands |"
    echo "| --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .compatibility.runs[] | "| \($cell) | \(.path) | \(.transport) | \(.matches_oracle) | \(.replay_matches) | \(.commands_sent) |"' "$file"
    done
    echo
    for file in "${facts[@]}"; do
      jq -r '"- `\(.cell)`: scale changes the hashes \(.compatibility.scale_changes_the_hashes); the new script against the old server is rejected \(.compatibility.new_script_old_server.rejected) (\(.compatibility.new_script_old_server.error)); probes: \([.compatibility.probes[] | "\(.code) (\(.message))"] | join("; ")); the probed run stayed intact \(.compatibility.probes_left_the_run_intact)."' "$file"
    done
    ;;
  workload-history-capture)
    echo "The live run (cadence 60) and the replays from every snapshot to the next at 1, 2, 4 and 8 workers:"
    echo
    for file in "${facts[@]}"; do
      jq -r '"- `\(.cell)`: \(.platform.available_parallelism) CPUs, live at \(.live_workers) workers, run hash \(.live.run_hash), \(.live.kicks) kicks, \(.live.events) events, \(.live.snapshots) snapshots; \(.replay_summary.replays_matching) of \(.replay_summary.replays) replays match (hashes and events), every replay matches \(.replay_summary.every_replay_matches); memory \(.memory.total_bytes) bytes against \(.memory.predicted_bytes) predicted (ratio \(.memory.measured_over_predicted), within 10 percent \(.memory.within_10_percent)), \(.memory.ten_minutes_at_60_per_second_megabytes) MB for ten minutes at 60 ticks per second."' "$file"
    done
    echo
    echo "The always-on cost per cadence (0 captures the log and the events only):"
    echo
    echo "| Cell | Workers | Cadence | Batch | Ticks per second | Capture share of the run | Capture us per tick | Snapshot us each | Log bytes per tick | Event bytes per tick |"
    echo "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .cost[] | "| \($cell) | \(.workers) | \(.cadence) | \(.batch) | \(.ticks_per_second) | \(.capture_share_of_run) | \(.capture_us_per_tick) | \(.snapshot_us_each) | \(.log_bytes_per_tick) | \(.event_bytes_per_tick) |"' "$file"
    done
    echo
    echo "Causal queries by instrumented replay on the completed capture (20 queries spread over the run):"
    echo
    echo "| Cell | Harness workers | Answered | Hashes matching | Latency ms min | Latency ms mean | Latency ms max | Restore ms mean | Replayed ticks mean | Replayed ticks max | Example: kicks, neighbor ticks, component changes |"
    echo "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .causal_queries[] | "| \($cell) | \(.workers) | \(.answered) | \(.hashes_matching) | \(.latency_ms_min) | \(.latency_ms_mean) | \(.latency_ms_max) | \(.restore_ms_mean) | \(.replayed_ticks_mean) | \(.replayed_ticks_max) | \(.example.kicks_on_the_entity // "n/a"), \(.example.ticks_with_neighbors // "n/a"), \(.example.component_changes // "n/a") |"' "$file"
    done
    echo
    echo "The live world stepped by a paced host (one tick per millisecond) while queries run on harness worlds:"
    echo
    echo "| Cell | Workers | Alone matches direct | Host step us alone | Harness workers | Queried matches direct | Host step us queried | Host wall ms | Queries | Hashes matching | Latency ms mean | Latency ms max |"
    echo "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .live_under_queries as $l | $l.queried[] | "| \($cell) | \($l.workers) | \($l.alone_matches_direct) | \($l.host_step_us_alone) | \(.query_workers) | \(.queried_matches_direct) | \(.host_step_us) | \(.host_wall_ms) | \(.queries_answered) | \(.queries_hashes_matching) | \(.query_latency_ms_mean) | \(.query_latency_ms_max) |"' "$file"
    done
    echo
    echo "Retention at the end of the paced run:"
    echo
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .live_under_queries.retention | "- `\($cell)`: \(.retention_ticks) ticks of retention at cadence \(.cadence): \(.snapshots_retained) snapshots retained, oldest answerable tick \(.oldest_answerable_tick), tick \(.inside_tick) answered \(.inside_answered), tick \(.outside_tick) rejected \(.outside_rejected) with \(.outside_code // "no error") (\(.outside_message // "")); \(.retained_bytes) bytes retained."' "$file"
    done
    ;;
  headless-modes)
    echo "The headless and harness hosts per worker count, compared with the sequential reference (the binary without the presentation capability; the configuration with it at one worker, when it built):"
    echo
    echo "| Cell | Configuration | Workers | Headless matches | Ticks per second | Harness matches | Ticks per second while stepping | Paused ms | Extracted ticks |"
    echo "| --- | --- | --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .configuration as $configuration | .hosts[] | "| \($cell) | \($configuration) | \(.workers) | \(.headless.matches_reference) | \(.headless.ticks_per_second) | \(.harness.matches_reference) | \(.harness.ticks_per_second_while_stepping) | \(.harness.paused_ms) | \(.headless.extracted_ticks) |"' "$file"
      jq -r '.cell as $cell | (.other_configuration.child_hosts // {}) | .configuration as $configuration | (.hosts // [])[] | "| \($cell) | \($configuration) | \(.workers) | \(.headless.matches_reference) | \(.headless.ticks_per_second) | \(.harness.matches_reference) | \(.harness.ticks_per_second_while_stepping) | \(.harness.paused_ms) | \(.headless.extracted_ticks) |"' "$file"
    done
    echo
    echo "Dynamic imports of each configuration's binary (windowing, graphics and audio libraries flagged):"
    echo
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | "- `\($cell)` \(.configuration): \(.imports.format // "unreadable") imports \([.imports.libraries[]?.name] | join(", ")); windowing, graphics or audio library imported: \(if .imports.error then .imports.error else .imports.imports_a_windowing_graphics_or_audio_library end) (\([.imports.denied[]?.name] | join(", "))). Other configuration (\(.other_configuration.build.configuration)) built \(.other_configuration.build.succeeded) in \(.other_configuration.build.seconds) s\(if .other_configuration.imports then ": \(.other_configuration.imports.format) imports \([.other_configuration.imports.libraries[]?.name] | join(", ")); flagged: \(.other_configuration.imports.imports_a_windowing_graphics_or_audio_library) (\([.other_configuration.imports.denied[]?.name] | join(", ")))" else "; stderr tail: \(.other_configuration.build.stderr_tail // "")" end)."' "$file"
    done
    echo
    echo "Self-descriptions (not built; discovered by the other configuration; requested absent by the other configuration):"
    echo
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .descriptions | "- `\($cell)`: this build \(.this_build | tostring); other build discovered \(.other_build_discovered | tostring) (discovery found \(.other_build_discovery_found)); other build requested absent \(.other_build_requested_absent | tostring); requested absent equals this build \(.requested_absent_equals_this_build); discovered equals this build \(.discovered_equals_this_build)."' "$file"
    done
    echo
    echo "Session facts of the binary and of the non-interactive child (the child's hosts at one worker):"
    echo
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | "- `\($cell)` binary: \(.session | tostring). Discovery of the other configuration: \(.other_configuration.describe.discovery | tostring)."' "$file"
      jq -r '.cell as $cell | .non_interactive | if type == "object" then "- `\($cell)` non-interactive child (\(.context)): \(.child.session // .child.error | tostring); hosts match \([.child.hosts[]?.headless.matches_reference, .child.hosts[]?.harness.matches_reference] | tostring)." else "- `\($cell)` non-interactive child: \(.)" end' "$file"
    done
    echo
    echo "Many worlds: N worlds at one worker on N threads (world-level parallelism), and one world at N workers (intra-world) for contrast:"
    echo
    echo "| Cell | CPUs | Worlds | Workers per world | Every world matches | Total ticks per second | Per-world ticks per second mean | Ratio to N times single world | Within 15 percent | One world at N workers: ticks per second |"
    echo "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .platform.available_parallelism as $cpus | .many_worlds as $m | $m.world_level[] | .worlds as $n | "| \($cell) | \($cpus) | \(.worlds) | \(.workers_per_world) | \(.every_world_matches) | \(.total_ticks_per_second) | \(.per_world_ticks_per_second_mean) | \(.ratio_to_n_times_single_world) | \(.within_15_percent) | \([$m.intra_world[] | select(.workers_per_world == $n) | .ticks_per_second] | first // "n/a") |"' "$file"
    done
    echo
    for file in "${facts[@]}"; do
      jq -r '"- `\(.cell)`: the sequential reference runs at \([.many_worlds.sequential_reference_ticks_per_second[].ticks_per_second] | map(tostring) | join(", ")) ticks per second (one run per seed)."' "$file"
    done
    ;;
  workload-test-harness)
    echo "The suite of sixteen tests per run (every test two worlds from one fixture; the armed run expects one failure); the suite's wall time less the fixture's setup against the solo run, the two ideals and the longest solo test:"
    echo
    echo "| Cell | Configuration | Run | Test threads | Panic armed | Passed | Failed | Suite wall ms | Setup ms | Speedup over solo | Ratio to the wall ideal | Ratio to the worker time ideal | Ratio to the longest solo test | Restore us mean | Restore us max | Under 2 ms |"
    echo "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | (.runs // []) as $runs | (.fixture // []) as $fixture | (.parallelism.suites // [])[] | .label as $label | .configuration as $configuration | ($runs[] | select(.label == $label and .configuration == $configuration)) as $run | ($fixture[] | select(.label == $label and .configuration == $configuration)) as $f | "| \($cell) | \(.configuration) | \(.label) | \(.test_threads) | \(.panic_armed) | \($run.passed) | \($run.failed) | \(.suite_wall_ms) | \(.setup_ms) | \(.speedup_over_solo) | \(.ratio_to_wall_ideal) | \(.ratio_to_worker_time_ideal) | \(.ratio_to_longest_solo) | \($f.restore_us_mean) | \($f.restore_us_max) | \($f.under_2_ms) |"' "$file"
    done
    echo
    for file in "${facts[@]}"; do
      jq -r '"- `\(.cell)`: \(.platform.available_parallelism) CPUs, size \(.size), \(.ticks) ticks, profile \(.profile); every run passed except the armed one: \(.every_run_passed_except_the_armed_one); hashes identical across runs and builds: \(.hashes_identical_across_runs); longest solo test \(.parallelism.longest_solo_test_ms) ms (\(.parallelism.longest_solo_test)), sum of the solo tests \(.parallelism.sum_of_solo_tests_ms) ms, worker time estimate \(.parallelism.worker_time_estimate_ms) ms; builds \([.builds[] | "\(.configuration) \(.seconds) s succeeded \(.succeeded)"] | join(", "))."' "$file"
    done
    echo
    echo "Divergence reports of the seven injected tests, per run:"
    echo
    echo "| Cell | Configuration | Run | Injected tests | Divergence found | Reported tick equals injected | Energy first | Only energy differs | Snapshots differ | Largest lag in ticks |"
    echo "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | (.divergence // [])[] | "| \($cell) | \(.configuration) | \(.label) | \(.injected_tests) | \(.divergence_found) | \(.reported_tick_equals_injected) | \(.energy_first) | \(.only_energy_differs) | \(.snapshots_differ) | \(.max_lag_ticks) |"' "$file"
    done
    echo
    echo "The injected tests of the solo run (injected and reported ticks, the first divergent group and system, the snapshots of that tick):"
    echo
    echo "| Cell | Test | Injected tick | Reported tick | First divergent group | System | Groups differing | Snapshot bytes | First differing byte |"
    echo "| --- | --- | --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | (.divergence // [])[] | select(.label == "solo") | .tests[] | "| \($cell) | \(.name) | \(.injected_tick) | \(.reported_tick) | \(.first_divergent_group) | \(.first_divergent_system) | \(.groups_differing) | \(.snapshot_bytes) | \(.first_differing_byte) |"' "$file"
    done
    echo
    echo "The armed run (a panic injected in a system of one test's second world):"
    echo
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .panic_run | "- `\($cell)`: ran \(.ran), exit failure \(.exit_failure // "n/a"), \(.passed // "n/a") passed and \(.failed // "n/a") failed, failed tests: \(.failed_tests // "none"), failed alone \(.failed_alone // "n/a"); failure reported at tick \(.failure_tick // "n/a") in \(.failure_node // "n/a") chunk \(.failure_chunk // "n/a") on worker \(.failure_worker // "n/a"), world A matching the reference \(.world_a_matches_reference // "n/a"); message: \(.message // "")."' "$file"
    done
    ;;
  *)
    echo "No summary is defined for this spike; the raw facts are in the artifacts."
    ;;
esac
