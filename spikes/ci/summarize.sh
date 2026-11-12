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
  control-surface-editor-rate)
    echo "Verdicts per cell (computed by the program; a timing verdict is a measurement, the checks are the hashes against the oracle and the deliveries):"
    echo
    for file in "${facts[@]}"; do
      jq -r '.hypotheses as $h | "- `\(.cell)`: \(.platform.available_parallelism) CPUs, \(.entity_count) entities at \(.tick_rate) ticks per second; every world matches its oracle and every delivery check passes: \(.every_check_passes). Stepping thread on behalf of the session under \($h.stepping_thread_share.threshold_percent) percent of the tick: \(if $h.stepping_thread_share.holds then "holds" else "fails" end) (worst \($h.stepping_thread_share.worst_share_of_tick_percent) percent at \($h.stepping_thread_share.worst_at); \($h.stepping_thread_share.worst_share_with_the_extraction_percent) percent were the extraction charged to the session; the run with the session against the run that only extracts: \($h.stepping_thread_share.worst_with_minus_extraction_alone_share_percent) percent at worst, against the run without extraction: \($h.stepping_thread_share.worst_with_minus_without_share_percent) percent at worst, both within the run-to-run variation of a paced tick). Set to live value within \($h.set_to_live_value_latency.threshold_ticks) ticks: \(if $h.set_to_live_value_latency.holds then "holds" else "fails" end) (worst 99th percentile \($h.set_to_live_value_latency.worst_p99_ms) ms, \($h.set_to_live_value_latency.worst_p99_ticks) ticks, at \($h.set_to_live_value_latency.worst_at); worst maximum \($h.set_to_live_value_latency.worst_max_ms) ms, within the threshold \($h.set_to_live_value_latency.maximum_within_threshold)). Change set size follows the fraction, not the tick count, and stays under the full-state read: \(if $h.change_set_size.holds then "holds" else "fails" end) (grows with the fraction \($h.change_set_size.grows_with_the_fraction); second half over first half \($h.change_set_size.worst_second_half_over_first_half) at worst; \($h.change_set_size.largest_change_set_bytes_at_100_percent) bytes at 100 percent against \($h.change_set_size.full_state_read_bytes) for the full-state read). Agent loss under \($h.agent_interference.threshold_percent) percent beside the editor session: \(if $h.agent_interference.holds then "holds" else "fails" end) (worst \($h.agent_interference.worst_loss_percent) percent at \($h.agent_interference.worst_at_workers) worker(s); shared resource: \($h.agent_interference.shared_resource))."' "$file"
    done
    echo
    echo "The stepping thread per fraction (microseconds, medians over the ticks; the session's part is the admission of its commands and the publication of the change bitset):"
    echo
    echo "| Cell | Workers | Fraction percent | Transport | Matches the oracle | Tick us without session | Tick us with extraction alone | Tick us with session | Session us | Extraction us | Session share of tick percent | Session share of period percent | With extraction share of tick percent | With minus extraction alone percent | With minus without percent |"
    echo "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .editor_runs[] | select(.with_session) | .with_session.stepping_thread as $s | "| \($cell) | \(.workers) | \(.fraction_percent) | \(.transport) | \(.with_session.matches_oracle and .with_session.outcomes_match_oracle and .without_session.matches_reference) | \(.without_session.tick_us_median) | \(.extraction_without_session.tick_us_median) | \($s.total_us_median) | \($s.session_us_median) | \($s.extraction_us_median) | \($s.session_share_of_tick_percent) | \($s.session_share_of_period_percent) | \($s.session_and_extraction_share_of_tick_percent) | \($s.with_minus_extraction_alone_share_of_tick_percent) | \($s.with_minus_without_share_of_tick_percent) |"' "$file"
    done
    echo
    echo "The drag and its latency (from a set command written by the client to the live-value message of the tick that decided it):"
    echo
    echo "| Cell | Workers | Fraction percent | Transport | Sets sent | Applied | Rejected stale | Latency ms median | Latency ms p99 | Latency ms max | Delivered ticks p99 | Delivered ticks max | Transaction applied | Inverse matches the oracle | I/O executor us per delivery median |"
    echo "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .editor_runs[] | select(.with_session) | .with_session as $w | $w.set_to_live_value_latency as $l | "| \($cell) | \(.workers) | \(.fraction_percent) | \(.transport) | \($w.drag.sets_sent) | \($w.drag.sets_applied) | \($w.drag.sets_rejected_stale) | \($l.ms_median) | \($l.ms_p99) | \($l.ms_max) | \($l.ticks_p99) | \($l.ticks_max) | \($w.drag.transaction_applied) | \($w.drag.inverse_matches_oracle) | \($w.io_executor.us_per_delivery_median) |"' "$file"
    done
    echo
    echo "Message sizes per period (bytes, means over the deliveries; the change set as sent has canonical rows and string names):"
    echo
    echo "| Cell | Workers | Fraction percent | Transport | Rows mean | Ticks per delivery max | Change set as sent | Canonical rows, interned names | Tagged rows, string names | Tagged rows, interned names | Second half over first half | Full-state read, canonical | Full-state read, tagged strings | Live values, string names | Live values, interned names | Set command, string names | Set command, interned names | Client copy matches |"
    echo "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .editor_runs[] | select(.with_session) | .with_session as $w | $w.change_set as $c | "| \($cell) | \(.workers) | \(.fraction_percent) | \(.transport) | \($c.rows_mean) | \($c.ticks_per_delivery_max) | \($c.bytes_mean) | \($c.bytes_mean_canonical_rows_interned_names) | \($c.bytes_mean_tagged_rows_string_names) | \($c.bytes_mean_tagged_rows_interned_names) | \($c.second_half_over_first_half) | \($w.full_state_read_bytes.canonical_rows_string_names) | \($w.full_state_read_bytes.tagged_rows_string_names) | \($w.live_values.bytes_mean_string_names) | \($w.live_values.bytes_mean_interned_names) | \($w.drag.set_command_bytes_string_names) | \($w.drag.set_command_bytes_interned_names) | \($c.client_copy_matches_host) |"' "$file"
    done
    echo
    echo "The stepped agent on a second world, alone and beside the editor session (ticks per second, the best of the rounds):"
    echo
    echo "| Cell | Workers | Every world matches its oracle | Alone | Beside, shared pool | Loss percent | Beside, one pool per world | Loss percent | Editor tick us alone | Editor tick us, shared pool | Shared resource |"
    echo "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .interference[] | "| \($cell) | \(.workers) | \(.every_world_matches_its_oracle) | \(.agent_alone_ticks_per_second) | \(.agent_beside_editor_shared_pool_ticks_per_second) | \(.loss_shared_pool_percent) | \(.agent_beside_editor_one_pool_per_world_ticks_per_second // "not run") | \(.loss_one_pool_per_world_percent // "not run") | \(.editor_tick_us_median_alone) | \(.editor_tick_us_median_shared_pool) | \(.shared_resource) |"' "$file"
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
  workload-lockstep)
    echo "Two peers in lockstep over the simulated link, every delay, link setting and worker count:"
    echo
    for file in "${facts[@]}"; do
      jq -r '"- `\(.cell)`: \(.platform.available_parallelism) CPUs; \(.lockstep | length) lockstep runs, \([.lockstep[] | select(.hashes_identical)] | length) with identical hashes at every tick, \([.lockstep[] | select(.matches_oracle)] | length) matching the oracle, \([.lockstep[] | select(.divergence_detected)] | length) divergence detected; delay 2 run hash \(.lockstep[0].run_hash), delay 4 run hash \([.lockstep[] | select(.delay == 4)][0].run_hash // "n/a")."' "$file"
    done
    echo
    echo "| Cell | Workers | Delay | Latency | Jitter | Loss | Identical | Oracle | Stall frames peer 0 | Stall frames peer 1 | Frames | Dropped | Bytes per tick per peer | Ticks per second, both worlds |"
    echo "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .lockstep[] | "| \($cell) | \(.workers) | \(.delay) | \(.latency) | \(.jitter_max) | \(.loss) | \(.hashes_identical) | \(.matches_oracle) | \(.stall_frames_peer0) | \(.stall_frames_peer1) | \(.frames) | \(.messages_dropped) | \(.bytes_per_tick_per_peer) | \(.ticks_per_second_both_worlds) |"' "$file"
    done
    echo
    echo "The injected desync (a kick peer 1 applies and never sends) and the same run without it:"
    echo
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .divergence | "- `\($cell)`: injected at tick \(.injected_tick), \(.workers) workers, delay \(.delay), latency \(.latency): detected \(.with_injection.detected) at tick \(.with_injection.detected_tick // "n/a") on frame \(.with_injection.detected_at_frame // "n/a") by \(.with_injection.detected_by // "nobody"); first divergent group \(.with_injection.first_divergent_group // "n/a"); snapshots of the tick identical \(if .with_injection.snapshots_identical == null then "n/a" else .with_injection.snapshots_identical end), first differing byte \(.with_injection.first_differing_byte // "n/a"), \(.with_injection.snapshot_bytes_peer0 // "n/a") bytes each, \(.with_injection.snapshot_us_each) us per snapshot; without injection: detected \(.without_injection.detected), hashes identical \(.without_injection.hashes_identical), matches the oracle \(.without_injection.matches_oracle)."' "$file"
    done
    echo
    echo "The rollback variant on peer 1 (ring of 8 ticks past the confirmed tick), delay 2:"
    echo
    echo "| Cell | Workers | Latency | Jitter | Loss | Rollbacks | Predicted ticks | Mispredicted ticks | Re-simulated per rollback, mean | Max | Rollback ms mean | Max | Total | Stall frames peer 0 | Peer 1 | Confirmed hashes match | Oracle | Snapshot us each | Frames | Elapsed ms |"
    echo "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .rollback[] | "| \($cell) | \(.workers) | \(.latency) | \(.jitter_max) | \(.loss) | \(.rollbacks) | \(.predicted_ticks) | \(.mispredicted_ticks) | \(.resimulated_ticks_mean) | \(.resimulated_ticks_max) | \(.rollback_ms_mean) | \(.rollback_ms_max) | \(.rollback_ms_total) | \(.stall_frames_peer0) | \(.stall_frames_peer1) | \(.confirmed_hashes_match) | \(.matches_oracle) | \(.snapshot_us_each) | \(.frames) | \(.elapsed_ms) |"' "$file"
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
  workload-render-extraction)
    echo "Every run (the normal variant against the sequential reference; the static-half variant against itself at one worker without extraction):"
    echo
    echo "| Cell | Variant | Mode | Workers | Matches | First divergent tick | Render run hash | Ticks per second | Ratio to baseline | Extraction us per tick | Share of the tick | Chunks gathered per tick | Chunks copied per tick |"
    echo "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .runs[] | "| \($cell) | \(.variant) | \(.mode) | \(.workers) | \(.matches_reference // .matches_one_worker) | \(.first_divergent_tick) | \(.render_run_hash) | \(.ticks_per_second) | \(.ratio_to_baseline) | \(.extraction_us_per_tick) | \(.extraction_share_of_tick) | \(.chunks_gathered_per_tick) | \(.chunks_copied_per_tick) |"' "$file"
    done
    echo
    echo "Distinct render state run hashes per variant across every cell, mode and worker count (1 means identical everywhere):"
    echo
    for variant in $(jq -r '.runs[].variant' "${facts[@]}" | sort -u); do
      distinct="$(jq -r --arg variant "$variant" '.runs[] | select(.variant == $variant and .mode != "off") | .render_run_hash' "${facts[@]}" | sort -u | wc -l | tr -d ' ')"
      echo "- \`$variant\`: $distinct distinct render run hash value(s)."
    done
    echo
    echo "Per cell: debug assertions, available CPUs, and the runs that match:"
    echo
    for file in "${facts[@]}"; do
      jq -r '"- `\(.cell)`: debug assertions \(.build.debug_assertions), \(.platform.available_parallelism) CPUs, \([.runs[] | select(.matches_reference // .matches_one_worker)] | length) of \(.runs | length) runs match; the sequential reference runs at \(.reference_ticks_per_second) ticks per second."' "$file"
    done
    ;;
  plugin-tiers)
    echo "The motion pass through each tier, per worker count, against the sequential reference (the built-in pass is the kernel inlined in the world):"
    echo
    echo "| Cell | Tier | Form | Available | Workers | Matches | First divergent tick | Ticks per second | Motion us per tick | Motion us per chunk | Ratio to built-in | Copy us per tick (module) |"
    echo "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .tiers[] | "| \($cell) | \(.tier) | \(.form) | \(.available) | \(.workers // "n/a") | \(.matches_reference // "n/a") | \(.first_divergent_tick // "n/a") | \(.ticks_per_second // "n/a") | \(.motion_us_per_tick // "n/a") | \(.motion_us_per_chunk // "n/a") | \(.ratio_to_builtin // "n/a") | \(.copy_us_per_tick // "n/a") |"' "$file"
    done
    echo
    echo "Distinct run hashes per tier and form across every cell and worker count (1 means identical everywhere; the reference run hash is listed first):"
    echo
    echo "- reference: $(jq -r '.reference_run_hash' "${facts[@]}" | sort -u | wc -l | tr -d ' ') distinct value(s)."
    for config in $(jq -r '.tiers[] | select(.available) | "\(.tier)/\(.form)"' "${facts[@]}" | tr ' ' '_' | sort -u); do
      distinct="$(jq -r --arg config "$config" '.tiers[] | select(.available) | select(("\(.tier)/\(.form)" | gsub(" "; "_")) == $config) | .run_hash' "${facts[@]}" | sort -u | wc -l | tr -d ' ')"
      matching="$(jq -r --arg config "$config" '.tiers[] | select(.available) | select(("\(.tier)/\(.form)" | gsub(" "; "_")) == $config) | select(.matches_reference) | .run_hash' "${facts[@]}" | wc -l | tr -d ' ')"
      total="$(jq -r --arg config "$config" '.tiers[] | select(.available) | select(("\(.tier)/\(.form)" | gsub(" "; "_")) == $config) | .run_hash' "${facts[@]}" | wc -l | tr -d ' ')"
      echo "- \`$(echo "$config" | tr '_' ' ')\`: $distinct distinct run hash value(s), $matching of $total runs match the reference."
    done
    echo
    echo "Reloads between two steps (the module instantiated again from its file; a library unloaded and loaded again):"
    echo
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .reloads[] | "- `\($cell)` \(.tier) (\(.form)) at \(.workers) workers: reloaded \(.reloaded) in \(.reload_ms // "n/a") ms at tick \(.reload_tick); matches the reference \(.matches_reference); the reload changes the hash: \(.reload_changes_hash // "n/a")\(if .failure then "; failure: \(.failure)" else "" end)."' "$file"
    done
    echo
    echo "Run-time builds per cell:"
    echo
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .builds[] | "- `\($cell)` \(.what): succeeded \(.succeeded) in \(.seconds) s; compiler \(.compiler); flags \(.flags)\(if .succeeded then "" else "; \(.stderr_tail | gsub("\n"; " ") | .[0:300])" end)."' "$file"
    done
    echo
    echo "Tiers unavailable on a cell, with the reason:"
    echo
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .tiers[] | select(.available | not) | "- `\($cell)` \(.tier) (\(.form)): \(.reason | gsub("\n"; " ") | .[0:300])"' "$file"
    done
    echo
    echo "The out-of-process importer (spawn plus round trip, five samples):"
    echo
    for file in "${facts[@]}"; do
      jq -r '"- `\(.cell)`: \(.importer.payload_bytes) bytes, content hash \(.importer.content_hash), every sample matches \(.importer.matches_expected); spawn and round trip us: \([.importer.round_trips[] | .spawn_and_round_trip_us // "error"] | map(tostring) | join(", "))."' "$file"
    done
    ;;
  scripting-runtime)
    echo "Every gameplay system of the tick through the native baseline, the compiling runtime and the interpreter (a run with the cosine and sine compiled into the module has no platform reference):"
    echo
    echo "| Cell | Runtime | Workers | Wander | Ran | Matches the reference | First divergent tick | Run hash | ms per tick | Systems us per tick (summed over workers) | Copies us per tick | Serial us per tick | Fuel hash | Largest chunk call fuel |"
    echo "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | (.runs // [])[] | "| \($cell) | \(.runtime) | \(.workers) | \(.wander) | \(.ran) | \(.matches_reference | if . == null then "n/a" else . end) | \(.first_divergent_tick | if . == null then "n/a" else . end) | \(.run_hash | if . == null then "n/a" else . end) | \(.ms_per_tick | if . == null then "n/a" else . end) | \(.call_us_per_tick | if . == null then "n/a" else . end) | \(.copy_us_per_tick | if . == null then "n/a" else . end) | \(.serial_us_per_tick | if . == null then "n/a" else . end) | \(.fuel_hash | if . == null then "n/a" else . end) | \(.largest_chunk_call_fuel | if . == null then "n/a" else . end) |\(if .failure then " \(.failure | gsub("\n"; " ") | .[0:200])" else "" end)"' "$file"
    done
    echo
    echo "Distinct values across every cell (1 means identical everywhere):"
    echo
    echo "- reference run hash, wander off: $(jq -r '.reference_run_hash_wander_off' "${facts[@]}" | sort -u | wc -l | tr -d ' '); wander on (the platform's cosine and sine): $(jq -r '.reference_run_hash_wander_on' "${facts[@]}" | sort -u | wc -l | tr -d ' ')."
    for wander in "off" "compiled into the module" "host functions"; do
      echo "- wander $wander, run hash over the module runs of both runtimes and every worker count: $(jq -r --arg wander "$wander" '(.runs // [])[] | select(.ran and .wander == $wander and .runtime != "native") | .run_hash' "${facts[@]}" | sort -u | wc -l | tr -d ' ')."
      for runtime in "wasmtime, fuel on" "wasmi, fuel on"; do
        echo "- wander $wander, fuel hash of \`$runtime\` over every worker count: $(jq -r --arg wander "$wander" --arg runtime "$runtime" '(.runs // [])[] | select(.ran and .wander == $wander and .runtime == $runtime) | .fuel_hash' "${facts[@]}" | sort -u | wc -l | tr -d ' ')."
      done
      echo "- wander $wander, fuel hash over both runtimes: $(jq -r --arg wander "$wander" '(.runs // [])[] | select(.ran and .wander == $wander and .metered) | .fuel_hash' "${facts[@]}" | sort -u | wc -l | tr -d ' ')."
    done
    echo "- module content hashes: first $(jq -r '.builds[0].content_hash // "none"' "${facts[@]}" | sort -u | wc -l | tr -d ' '), second $(jq -r '.builds[1].content_hash // "none"' "${facts[@]}" | sort -u | wc -l | tr -d ' ')."
    echo "- reload run hash over every runtime and worker count: $(jq -r '(.reloads // [])[] | select(.ran) | .run_hash' "${facts[@]}" | sort -u | wc -l | tr -d ' ')."
    echo
    for file in "${facts[@]}"; do
      jq -r '"- `\(.cell)`: \(.platform.available_parallelism) CPUs; \([(.runs // [])[] | select(.matches_reference == true)] | length) of \([(.runs // [])[] | select(.matches_reference != null)] | length) runs with a reference match it, \([(.runs // [])[] | select(.ran | not)] | length) did not run; the sequential reference runs at \(.reference_ticks_per_second_wander_off) ticks per second; module builds: \([.builds[] | "\(.succeeded) in \(.seconds) s"] | join(", "))\(if .runtimes_failure then "; runtimes failure: \(.runtimes_failure)" else "" end)."' "$file"
    done
    echo
    echo "Invalid floating-point results (bit patterns):"
    echo
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | (.invalid_results // [])[] | "- `\($cell)` \(.runtime): \(.zero_over_zero // .failure), \(.infinity_minus_infinity // ""), \(.square_root_of_negative // ""), \(.zero_times_infinity // "")."' "$file"
    done
    echo
    echo "Injected faults (chunk 17 of tick 300 at 4 workers):"
    echo
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | (.injected_faults // [])[] | "- `\($cell)` \(.runtime), \(.fault): failed \(.failed // .failure) at tick \(.failure_tick | if . == null then "n/a" else . end) in chunk \(.failure_chunk | if . == null then "n/a" else . end) with \(.trap | if . == null then "n/a" else . end), fuel consumed by the failing call \(.fuel_consumed_by_the_failing_call | if . == null then "n/a" else . end); last completed tick \(.last_completed_tick | if . == null then "n/a" else . end), hashes match up to there \(.hashes_match_reference_up_to_there | if . == null then "n/a" else . end), state is that tick \(.state_is_the_last_completed_tick | if . == null then "n/a" else . end), further step refused \(.further_step_refused | if . == null then "n/a" else . end)."' "$file"
    done
    echo
    echo "Fuel exhaustion on the first tick, and the fuel of a first and a second world on one runtime:"
    echo
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | (.fuel_exhaustion // [])[] | "- `\($cell)` \(.runtime): the largest chunk call consumes \(.largest_chunk_call_fuel_of_tick_1 // .failure); the smallest budget that completes the tick is \(.smallest_budget_that_completes_tick_1 | if . == null then "n/a" else . end) and gives the reference tick \(.that_budget_gives_the_reference_tick | if . == null then "n/a" else . end); one unit less stops the tick \(.one_unit_less_stops_the_tick | if . == null then "n/a" else . end) with \(.trap | if . == null then "n/a" else . end) in chunk \(.failure_chunk | if . == null then "n/a" else . end) after \(.fuel_consumed_by_the_failing_call | if . == null then "n/a" else . end), state untouched \(.state_untouched_after_the_stop | if . == null then "n/a" else . end)."' "$file"
      jq -r '.cell as $cell | (.fuel_of_a_first_and_a_second_world // [])[] | "- `\($cell)` \(.runtime // "not created"): first world \(.first_world_fuel_of_tick_1 // .failure), second world \(.second_world_fuel_of_tick_1 | if . == null then "n/a" else . end), equal \(.equal | if . == null then "n/a" else . end)."' "$file"
    done
    echo
    echo "Recursion, host calls and instantiation:"
    echo
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | (.recursion // [])[] | "- `\($cell)` \(.runtime): the deepest recursion that returns is \(.deepest_recursion_that_returns // .failure) levels (\(.fuel_at_that_depth | if . == null then "n/a" else . end) fuel), stopped by \(.trap // "the search cap"); \(.fuel_per_level | if . == null then "n/a" else . end) fuel per level."' "$file"
      jq -r '.cell as $cell | (.host_calls // [])[] | "- `\($cell)` \(.runtime): \(.ns_per_call_and_loop_iteration // .failure) ns and \(.fuel_per_call_and_loop_iteration | if . == null then "n/a" else . end) fuel per host call with its loop iteration."' "$file"
      jq -r '.cell as $cell | (.instantiation // [])[] | "- `\($cell)` \(.runtime): module compiled or translated in \(.module_compilation_ms // .failure) ms; an instance in \(.median_us | if . == null then "n/a" else . end) us (median), \(.mean_us | if . == null then "n/a" else . end) (mean), \(.slowest_us | if . == null then "n/a" else . end) (slowest of \(.instantiations | if . == null then "n/a" else . end)); memory \(.memory_bytes | if . == null then "n/a" else . end) bytes."' "$file"
    done
    echo
    echo "The second module loaded before tick 500:"
    echo
    echo "| Cell | Runtime | Workers | Ran | Run hash | First divergent tick from the reference | Matches the native oracle | Replay from the load record matches | Reload ms |"
    echo "| --- | --- | --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | (.reloads // [])[] | "| \($cell) | \(.runtime) | \(.workers | if . == null then "n/a" else . end) | \(.ran) | \(.run_hash | if . == null then "n/a" else . end) | \(.first_divergent_tick_from_the_reference | if . == null then "n/a" else . end) | \(.matches_the_native_oracle | if . == null then "n/a" else . end) | \(.replay_from_the_load_record_matches | if . == null then "n/a" else . end) | \(.reload_ms | if . == null then "n/a" else . end) |\(if .failure then " \(.failure | gsub("\n"; " ") | .[0:200])" else "" end)"' "$file"
    done
    ;;
  workload-asset-arrival)
    echo "The loading gate: a world that requires the asset waits for the loader (a real delay that differs between runs) and then steps; the hashes must be the reference's whatever the delay:"
    echo
    echo "| Cell | Workers | Delay | Requested ms | Loader ms | Gate wait ms | Step while loading | Arrival tick | Gate opened at | Run hash | Matches the reference | First divergent tick | Ticks per second |"
    echo "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .gate[] | "| \($cell) | \(.workers) | \(.delay) | \(.delay_ms_requested) | \(.delay_ms_measured) | \(.gate_wait_ms) | \(.step_while_loading) | \(.arrival_tick) | \(.gate_opened_at) | \(.run_hash) | \(.matches_reference) | \(.first_divergent_tick) | \(.ticks_per_second) |"' "$file"
    done
    echo
    distinct="$(jq -r '.gate[].run_hash' "${facts[@]}" | sort -u | wc -l | tr -d ' ')"
    echo "- Distinct gate run hashes across every cell, delay and worker count (1 means identical everywhere): $distinct."
    for file in "${facts[@]}"; do
      jq -r '"- `\(.cell)`: \(.platform.available_parallelism) CPUs; the oracle with version 1 equals the reference \(.oracle_with_version_1_equals_reference); \([.gate[] | select(.matches_reference)] | length) of \(.gate | length) gated runs match the reference; version 1 content hash \(.tables.version_1_content_hash), version 2 \(.tables.version_2_content_hash)."' "$file"
    done
    echo
    echo "The declared default and the replay (the host steps at once; the arrival takes effect at a tick the real delay decides; that tick is an input, so the recorded log replays the run at every worker count and the sequential oracle switched at that tick agrees):"
    echo
    echo "| Cell | Workers | Delay ms | Arrival tick | Live run hash | Live matches the oracle | First divergent tick from the reference | Replay matches live (every worker count) | Replay run hash | Log bytes | Ticks per second |"
    echo "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .default_and_replay[] | "| \($cell) | \(.workers) | \(.delay_ms) | \(.arrival_tick) | \(.live_run_hash) | \(.live_matches_oracle) | \(.first_divergent_tick_from_reference) | \(.replay_matches_live) (\([.replay_workers[] | select(.matches_live)] | length) of \(.replay_workers | length)) | \(.replay_run_hash) | \(.log_bytes) | \(.ticks_per_second) |"' "$file"
    done
    echo
    for file in "${facts[@]}"; do
      jq -r '"- `\(.cell)`: the two default runs land at different ticks \(.default_runs_compared.arrival_ticks_differ) and give different run hashes \(.default_runs_compared.run_hashes_differ); every replay matches its live run \(.default_runs_compared.every_replay_matches_its_live_run)."' "$file"
    done
    echo
    echo "The fence removed (the control: the pass reads the table the moment the loader stores it; two runs per worker count, each against the oracle switched at the tick during which the new value was first read, then against each other):"
    echo
    echo "| Cell | Workers | Delay ms | Tick first observed | Run hash | Matches the oracle at that tick | First divergent tick | Matches the oracle one tick later | First divergent tick | Runs identical | First divergent tick between runs | Ticks per second |"
    echo "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .fence_removed[] | "| \($cell) | \(.workers) | \(.delay_ms) | \(.arrival_tick_observed) | \(.run_hash) | \(.matches_oracle) | \(.first_divergent_tick_from_oracle) | \(.matches_oracle_one_tick_later) | \(.first_divergent_tick_from_oracle_one_tick_later) | \(.runs_identical) | \(.first_divergent_tick_between_runs) | \(.ticks_per_second) |"' "$file"
    done
    echo
    echo "The hot reload (the gate mode with version 2 of the table applied at the fixed point of a scripted tick, the oracle switched there, the recorded log replayed):"
    echo
    echo "| Cell | Workers | Delay ms | Gate wait ms | Switch tick | Run hash | Matches the oracle | First divergent tick | Replay matches | Availability at the end | Log bytes | Ticks per second |"
    echo "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .hot_reload[] | "| \($cell) | \(.workers) | \(.delay_ms) | \(.gate_wait_ms) | \(.switch_tick) | \(.run_hash) | \(.matches_oracle) | \(.first_divergent_tick_from_oracle) | \(.replay_matches) | \(.availability_at_the_end | gsub("\\|"; "\\\\|")) | \(.log_bytes) | \(.ticks_per_second) |"' "$file"
    done
    echo
    distinct="$(jq -r '.hot_reload[].run_hash' "${facts[@]}" | sort -u | wc -l | tr -d ' ')"
    echo "- Distinct hot reload run hashes across every cell and worker count (1 means identical everywhere): $distinct."
    echo
    echo "The cost of the fence in two steps: the plain world (the kernel with the constants inlined, no asset), the parameterized world (the same kernel reading its table from a runtime value, no asset) and the fenced world (the table read from the domain state per chunk, the queue drained at every fixed point); parameterized against plain is the kernel's parameterization, fenced against parameterized is the fence:"
    echo
    echo "| Cell | Workers | Rounds | Plain (best) | Parameterized (best) | Fenced (best) | Fenced to plain | Parameterized to plain | Fenced to parameterized | Every run matches | Rounds (plain, parameterized, fenced) |"
    echo "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .cost[] | "| \($cell) | \(.workers) | \(.rounds) | \(.plain_ticks_per_second) | \(.parameterized_ticks_per_second // "n/a") | \(.fenced_ticks_per_second) | \(.ratio) | \(.ratio_parameterized_to_plain // "n/a") | \(.ratio_fenced_to_parameterized // "n/a") | \(.every_run_matches_reference) | \([.rounds_ticks_per_second[] | "\(.plain), \(.parameterized // "n/a"), \(.fenced)"] | join("; ")) |"' "$file"
    done
    echo
    echo "Failed transitions (an unknown asset, a payload of the wrong length and a payload with a wrong content hash, then a good payload):"
    echo
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .failed_transitions | "- `\($cell)`: \(.transitions_logged) transitions logged, \(.transitions_failed) failed (\([.reasons[] | select(.failed) | "tick \(.tick): \(.reason)"] | join("; "))); availability before the good payload \(.availability_before_the_good_payload); available at the end \(.available_at_the_end); matches the oracle switched at the tick of the good payload \(.matches_oracle)."' "$file"
    done
    ;;
  render-offscreen)
    echo "The scene (one triangle per alive entity of the small population after 100 ticks) rendered twice from scratch by every graphics API the machine offers at run time:"
    echo
    echo "| Cell | API | Available | Device | Type | Driver | Shader compiler | Frames identical | Image hash | Lit pixels | Device us | Pipeline us | Render us (first, second) | Readback us | Reason when unavailable |"
    echo "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .renderers[] | "| \($cell) | \(.api) | \(.available) | \(.device_name) | \(.device_type) | \(.driver) | \(.shader_compiler) | \(.frames_identical) | \(.image_hash) | \(.lit_pixels // "n/a") | \(.device_us) | \(.pipeline_us) | \([.frames[].render_us] | map(tostring) | join(", ")) | \([.frames[].readback_us] | map(tostring) | join(", ")) | \(.reason // "") |"' "$file"
    done
    echo
    echo "Distinct image hashes per API across every cell (1 means pixel-identical everywhere), and across every API and cell:"
    echo
    for api in $(jq -r '.renderers[] | select(.available) | .api' "${facts[@]}" | sort -u); do
      distinct="$(jq -r --arg api "$api" '.renderers[] | select(.available and .api == $api) | .image_hash' "${facts[@]}" | sort -u | wc -l | tr -d ' ')"
      cells="$(jq -r --arg api "$api" '.renderers[] | select(.available and .api == $api) | .api' "${facts[@]}" | wc -l | tr -d ' ')"
      echo "- \`$api\`: $distinct distinct image hash value(s) across $cells cell(s)."
    done
    all="$(jq -r '.renderers[] | select(.available) | .image_hash' "${facts[@]}" | sort -u | wc -l | tr -d ' ')"
    echo "- every available API on every cell: $all distinct image hash value(s)."
    scene="$(jq -r '.scene.vertex_buffer_hash' "${facts[@]}" | sort -u | wc -l | tr -d ' ')"
    echo "- the vertex buffer: $scene distinct hash value(s) across every cell (the scene is the same bytes everywhere)."
    echo
    echo "Session and imports per cell:"
    echo
    for file in "${facts[@]}"; do
      jq -r '"- `\(.cell)`: \(.platform.available_parallelism) CPUs; DISPLAY set \(.session.display_set), WAYLAND_DISPLAY set \(.session.wayland_display_set); open sockets before and after rendering: \(.session.open_sockets_before_rendering), \(.session.open_sockets_after_rendering); Vulkan ICD files: \([.session.vulkan_icd_files[].file] | join(", ")); GLSL compiler: \(.session.glsl_compiler); the binary (\(.imports.format)) imports \([.imports.libraries[]?.name] | join(", ")); a windowing, graphics or audio library imported: \(if .imports.error then .imports.error else .imports.imports_a_windowing_graphics_or_audio_library end)."' "$file"
    done
    ;;
  render-device-baseline)
    echo "Every device each graphics interface lists, opened with no window through the system's loader, and the scene with filtered textures, blending, depth and 4 samples rendered in three recordings on two devices of one process, in a second process and under each setting of a software device:"
    echo
    echo "| Cell | Interface | Device | Kind | Driver | Interface version | Opened | Every baseline item reported | Renders completed | Distinct scene hashes | Every render identical | Image hash | Nearest-filtered pixels off their texel | Compute exact | Pass-through exact | Converting form: differing bytes, largest | Reason when not opened |"
    echo "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .interfaces[] | .interface as $interface | .devices[]? | "| \($cell) | \($interface) | \(.name) | \(.kind) | \(.driver) | \(.interface_version) | \(.opened) | \(if .every_baseline_item_reported == null then "n/a" else .every_baseline_item_reported end) | \(.renders_completed // "n/a") of \(.renders_expected // "n/a") | \(.distinct_scene_hashes // "n/a") | \(if .every_render_identical == null then "n/a" else .every_render_identical end) | \(.scene.image_hash // "n/a") | \(.scene.nearest_filtered_pixels_that_differ_from_their_texel // "n/a") | \(if .compute.exact == null then (.compute.reason // "n/a") else .compute.exact end) | \(if .pass_through.pass_through == null then (.pass_through.reason // "n/a") else .pass_through.pass_through.every_texel_arrives_with_its_bytes end) | \(.pass_through.converting.differing_bytes // "n/a"), \(.pass_through.converting.largest_difference_per_channel // "n/a") | \(.reason // "") |"' "$file"
    done
    echo
    echo "Interfaces unavailable on a cell, with the reason:"
    echo
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .interfaces[] | select(.available | not) | "- `\($cell)`, \(.interface): \(.reason)"' "$file"
    done
    echo
    echo "Baseline items a device does not report, with what it reports instead (none listed means every item is reported):"
    echo
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .interfaces[] | .interface as $interface | .devices[]? | select(.opened) | .name as $device | .baseline[] | select(.reported != "yes") | "- `\($cell)`, \($interface), \($device): \(.name) is \(.reported) (\(.evidence))"' "$file"
    done
    echo
    echo "What each device reports beyond the baseline:"
    echo
    echo "| Cell | Interface | Device | BC | ETC2 | ASTC | 64-bit atomics | Indirect count from a buffer | Resource arrays without a fixed bound | Mesh shaders | Ray queries | Second queue |"
    echo "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .interfaces[] | .interface as $interface | .devices[]? | select(.opened) | "| \($cell) | \($interface) | \(.name) | \([.beyond_the_baseline[].reported] | join(" | ")) |"' "$file"
    done
    echo
    echo "The evidence behind each item, per device:"
    echo
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .interfaces[] | .interface as $interface | .devices[]? | select(.opened) | "- `\($cell)`, \($interface), \(.name) (settings: \(.settings)): " + ([(.baseline + .beyond_the_baseline)[] | "\(.name): \(.evidence)"] | join("; "))' "$file"
    done
    echo
    echo "The three recordings on the first and on the second device of the process (render time, read-back time and polls, time between the two timestamps):"
    echo
    echo "| Cell | Interface | Device | Recording | First hash | Second hash | Render us (first, second) | Read-back us (first, second) | Read-back polls (first, second) | Timestamps ns (first, second) | Error |"
    echo "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .interfaces[] | .interface as $interface | .devices[]? | select(.opened) | .name as $device | .scene.recordings[] | "| \($cell) | \($interface) | \($device) | \(.recording) | \(.first_device_hash // "n/a") | \(.second_device_hash // "n/a") | \(.first_device_render_us // "n/a"), \(.second_device_render_us // "n/a") | \(.first_device_readback_us // "n/a"), \(.second_device_readback_us // "n/a") | \(.first_device_readback_polls // "n/a"), \(.second_device_readback_polls // "n/a") | \(.first_device_timestamps_ns // "n/a"), \(.second_device_timestamps_ns // "n/a") | \([.first_device_error, .second_device_error] | map(select(. != null)) | join("; ")) |"' "$file"
    done
    echo
    echo "The scene again in child processes (the same settings, then each setting of a software device), with the difference from the first image where the hash differs:"
    echo
    echo "| Cell | Interface | Device | Run | What | Environment | Processors | Completed | Device as the child names it | Threads | Hashes (one buffer, chunked on one thread, chunked on four) | Differences |"
    echo "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .interfaces[] | .interface as $interface | .devices[]? | select(.opened) | .name as $device | .runs[] | "| \($cell) | \($interface) | \($device) | \(.label) | \(.what) | \(.environment) | \(.affinity_cpus) | \(.completed) (\(.status)) | \(.device // "n/a") | \(.threads // "n/a") | \([.recordings[] | .hash // .error] | join(", ")) | \([.recordings[] | select(.differing_pixels != null) | "\(.recording): \(.differing_pixels) pixels, largest \(.largest_difference_per_channel)"] | join("; ")) |"' "$file"
    done
    echo
    echo "Distinct images of the scene per interface and device name across every cell (1 means identical bytes on every cell that has the device), and the differences between the devices of one cell:"
    echo
    jq -r '.interfaces[] | .interface as $interface | .devices[]? | select(.scene.image_hash != null) | "\($interface)\t\(.name)\t\(.scene.image_hash)"' "${facts[@]}" | sort -u | awk -F '\t' '{ key = $1 ", " $2; count[key] += 1; hashes[key] = hashes[key] " " $3 } END { for (key in count) print "- " key ": " count[key] " distinct image(s):" hashes[key] }' | sort
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .differences_between_devices[] | "- `\($cell)`: \(.first) and \(.second): \(.differing_pixels // .difference) differing pixels, \(.differing_bytes // "n/a") differing bytes, largest difference per channel \(.largest_difference_per_channel // "n/a")"' "$file"
    done
    inputs="$(jq -r '.inputs.hash' "${facts[@]}" | sort -u | wc -l | tr -d ' ')"
    echo "- the inputs: $inputs distinct hash value(s) across every cell (the scenes are the same bytes everywhere)."
    echo
    echo "The validation layer of each interface, in a child process: whether it could be used, how its messages reach the program, the messages while the scene renders, and whether a deliberate misuse is reported:"
    echo
    echo "| Cell | Interface | Device | Child completed | Layer | Delivery | Scene rendered | Messages on the scene | Misuse | Messages on the misuse | Reported to the program | First message, reason or standard error |"
    echo "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .interfaces[] | .interface as $interface | .devices[]? | select(.opened) | .name as $device | .validation | "| \($cell) | \($interface) | \($device) | \(.completed) (\(.status)) | \(if .layer == null then "n/a" else .layer end) | \(.delivery // "n/a") | \(if .scene_rendered == null then "n/a" else .scene_rendered end) | \(.clean_messages // "n/a") | \(.misuse // "n/a") | \(.misuse_messages // "n/a") | \(.the_misuse_is_reported_to_the_program) | \(.misuse_first // .clean_first // .reason // .error // .standard_error // "") |"' "$file"
    done
    echo
    echo "The lit scene, timed from the start of the recording to the completion of the device's work (fastest and median of the repetitions, in microseconds):"
    echo
    echo "| Cell | Interface | Device | Kind | One image, lit | Geometry buffer of 160 bits, fill | Geometry buffer of 192 bits, fill | Geometry buffer of 160 bits, fill then light | Output hashes |"
    echo "| --- | --- | --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .interfaces[] | .interface as $interface | .devices[]? | select(.opened) | "| \($cell) | \($interface) | \(.name) | \(.kind) | \([.lit_scene[] | if .ran then "\(.fastest_us), \(.median_us)" else "not run: \(.reason)" end] | join(" | ")) | \([.lit_scene[] | .output_hash // "n/a"] | join(", ")) |"' "$file"
    done
    echo
    echo "Session and imports per cell:"
    echo
    for file in "${facts[@]}"; do
      jq -r '"- `\(.cell)`: \(.platform.available_parallelism) CPUs; \(.devices_measured) device(s) measured; DISPLAY set \(.session.display_set), WAYLAND_DISPLAY set \(.session.wayland_display_set); open sockets before and after rendering: \(.session.open_sockets_before_rendering), \(.session.open_sockets_after_rendering); threads after rendering: \(.session.threads_after_rendering); the binary (\(.imports.format)) imports \(.imports.libraries // .imports.error); a windowing, graphics or audio library imported: \(if .imports.error then .imports.error else .imports.imports_a_windowing_graphics_or_audio_library end)."' "$file"
    done
    ;;
  workload-persistent-pool)
    echo "The cost of a one-tick step on both pools, per worker count (the wall time of 1 000 calls of step(1) minus one call of step(1000), per step; the best of the rounds):"
    echo
    echo "| Cell | Pool | Workers | 1 000 calls ms | One batch ms | Overhead us per step | Ticks per second |"
    echo "| --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | (.throughput.runs // []) as $rates | .step_cost.runs[] | .pool as $pool | .workers as $workers | ($rates[] | select(.pool == $pool and .workers == $workers)) as $rate | "| \($cell) | \(.pool) | \(.workers) | \(.thousand_calls_ms) | \(.one_batch_ms) | \(.overhead_us_per_step) | \($rate.ticks_per_second) |"' "$file"
    done
    echo
    echo "| Cell | Workers | Step-scoped overhead us | Persistent overhead us | Ratio step-scoped over persistent | Throughput ratio persistent over step-scoped | Within 3 percent |"
    echo "| --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | (.throughput.comparisons // []) as $rates | .step_cost.ratios[] | .workers as $workers | ($rates[] | select(.workers == $workers)) as $rate | "| \($cell) | \(.workers) | \(.step_scoped_us) | \(.persistent_us) | \(.ratio_step_scoped_over_persistent) | \($rate.ratio_persistent_over_step_scoped) | \($rate.within_3_percent) |"' "$file"
    done
    echo
    for file in "${facts[@]}"; do
      jq -r '"- `\(.cell)`: \(.platform.available_parallelism) CPUs, size \(.size), \(.ticks) ticks, profile \(.profile), \(.rounds) rounds, spin bound \(.spin_bound_us) us, one hash of the state \(.step_cost.state_hash_us) us (the serial task a one-tick step cannot overlap); at 4 workers (measured \(.step_cost.at_four_workers.measured)) the persistent overhead is \(.step_cost.at_four_workers.persistent_us) us against \(.step_cost.at_four_workers.step_scoped_us) us step-scoped, under 50 us \(.step_cost.at_four_workers.under_50_us), under a fifth of the step-scoped \(.step_cost.at_four_workers.under_fifth_of_step_scoped): the step cost hypothesis holds \(.step_cost.holds); the throughput hypothesis holds \(.throughput.holds); the sequential reference runs at \(.reference_ticks_per_second) ticks per second."' "$file"
    done
    echo
    echo "The hashes on the persistent pool (every world against the sequential reference of its seed):"
    echo
    for file in "${facts[@]}"; do
      jq -r '"- `\(.cell)`: \([.hashes.batches[] | select(.matches_reference)] | length) of \(.hashes.batches | length) batch runs match (worker counts \([.hashes.batches[].workers] | unique | map(tostring) | join(", ")); batches \([.hashes.batches[].batch] | unique | map(tostring) | join(", "))); two worlds alternating: \([.hashes.two_worlds[] | select(.first_matches and .second_matches)] | length) of \(.hashes.two_worlds | length) runs match both; sixteen worlds from sixteen threads: \([.hashes.sixteen_worlds[] | select(.all_match)] | length) of \(.hashes.sixteen_worlds | length) runs match every world; the hypothesis holds \(.hashes.holds)."' "$file"
    done
    echo
    echo "The step after a pause (1 000 one-tick steps, each after the pause; the step's own wall time; the best round by the mean):"
    echo
    echo "| Cell | Pool | Workers | Pause ms | Step us mean | Step us max | Batch us per tick | Excess us mean |"
    echo "| --- | --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .wake_after_pause.runs[] | "| \($cell) | \(.pool) | \(.workers) | \(.pause_ms) | \(.step_us_mean) | \(.step_us_max) | \(.batch_us_per_tick) | \(.excess_us_mean) |"' "$file"
    done
    echo
    for file in "${facts[@]}"; do
      jq -r '"- `\(.cell)`: the persistent pool is cheaper after every pause at every worker count \(.wake_after_pause.persistent_cheaper_everywhere); its excess over the batch per tick at 4 workers is under 100 us \(.wake_after_pause.excess_under_100_us_at_4_workers) (\([.wake_after_pause.comparisons[] | select(.workers == 4) | "\(.persistent_excess_us) us after \(.pause_ms) ms"] | join(", "))); the hypothesis holds \(.wake_after_pause.holds)."' "$file"
    done
    echo
    echo "The idle pool after a step (one second; the spin and the parks of every persistent worker over the second):"
    echo
    echo "| Cell | Workers | Max spin us per worker | Max parks per worker | Workers already parked when the step returned | Every worker parked at the end | Holds |"
    echo "| --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .idle.runs[] | "| \($cell) | \(.workers) | \(.max_spin_us) | \(.max_parks) | \(.workers_parked_at_step_end) of \(.workers_stats | length) | \(.every_worker_parked_at_end) | \(.holds) |"' "$file"
    done
    echo
    echo "The harness pattern (sixteen worlds of one seed, each stepped 1 000 ticks one per call by its own thread):"
    echo
    echo "| Cell | Run | Pool | Pools | Workers per pool | Wall ms | Worlds matching |"
    echo "| --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .harness_pattern.worlds as $worlds | .harness_pattern.runs[] | "| \($cell) | \(.label) | \(.pool) | \(.pools) | \(.workers) | \(.wall_ms) | \(.matching) of \($worlds) |"' "$file"
    done
    echo
    for file in "${facts[@]}"; do
      jq -r '"- `\(.cell)`: the shared pool of the parallelism takes \(.harness_pattern.ratio_shared_over_step_scoped) times the wall time of the sixteen step-scoped pools (not slower by more than 10 percent: \(.harness_pattern.shared_not_slower_by_10_percent)); the hypothesis holds \(.harness_pattern.holds); the processor time is \(.harness_pattern.processor_time)."' "$file"
    done
    echo
    echo "The fault isolation on a shared pool (two worlds alternating, a panic injected in the second world's repulsion pass):"
    echo
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .fault_isolation.runs[] | "- `\($cell)` at \(.workers) workers: failure at tick \(.failure_tick) (injected \(.injected_tick)) in \(.failure_node) chunk \(.failure_chunk) on worker \(.failure_worker); the healthy world matches the reference \(.healthy_matches); the failed world stopped at tick \(.failed_tick_done) with its history matching \(.failed_history_matches), refuses a further step \(.failed_refuses_step); the pool serves a fresh world afterwards \(.pool_serves_after); holds \(.holds)."' "$file"
    done
    ;;
  asset-import-parallel)
    echo "Byte-identical across runs and worker counts: three imports per worker count, each from an empty cache; the manifest of a kind is the sorted list of asset identifier and content hash, hashed as a whole:"
    echo
    echo "| Cell | Workers | Runs | Distinct manifests across the runs (plain, geometric, control) | Manifest of every asset (run 1) | Wall ms (runs) |"
    echo "| --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .identical_across_runs_and_workers.per_worker_count[] | "| \($cell) | \(.workers) | \(.runs | length) | \([.runs[].manifest_plain] | unique | length), \([.runs[].manifest_geometric] | unique | length), \([.runs[].manifest_control] | unique | length) | \(.runs[0].manifest_all) | \([.runs[].wall_ms] | map(tostring) | join(", ")) |"' "$file"
    done
    echo
    for file in "${facts[@]}"; do
      jq -r '"- `\(.cell)`: \(.platform.available_parallelism) CPUs; \(.set.total) assets (\(.set.plain) plain, \(.set.geometric) geometric, \(.set.control) control), \(.set.source_bytes) source bytes, \(.output_bytes) output bytes; distinct manifests across every run and worker count: plain \(.identical_across_runs_and_workers.distinct_plain), geometric \(.identical_across_runs_and_workers.distinct_geometric), control \(.identical_across_runs_and_workers.distinct_control); failed actions \(.identical_across_runs_and_workers.failures); holds \(.identical_across_runs_and_workers.holds)."' "$file"
    done
    echo
    echo "Byte-identical across cells (the plain and geometric kinds must give one value; the control kind, normalized with the platform math library, is reported for information):"
    echo
    echo "| Cell | Plain manifest | Geometric manifest | Control manifest |"
    echo "| --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '"| \(.cell) | \(.manifest_plain) | \(.manifest_geometric) | \(.manifest_control) |"' "$file"
    done
    echo
    for kind in plain geometric control; do
      distinct="$(jq -r ".manifest_$kind" "${facts[@]}" | sort -u | wc -l | tr -d ' ')"
      echo "- \`$kind\`: $distinct distinct manifest hash value(s) across ${#facts[@]} cell(s)."
    done
    echo
    echo "Incremental re-import (a cache mirrored on disk; after the full import, each re-import must import exactly the expected actions and leave the untouched assets' content hashes unchanged):"
    echo
    echo "| Cell | Change | Expected actions | Actions imported | Exactly those | Memory hits | Disk hits | Untouched unchanged | Elapsed ms | Ratio to the full import | Holds |"
    echo "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .incremental.re_imports[] | "| \($cell) | \(.change) | \(.expected_actions) | \(.actions_imported) | \(.exactly_the_expected_actions) | \(.cache_memory_hits) | \(.cache_disk_hits) | \(.untouched_hashes_unchanged) | \(.elapsed_ms) | \(.ratio_to_full_import) | \(.holds) |"' "$file"
    done
    echo
    for file in "${facts[@]}"; do
      jq -r '"- `\(.cell)`: the full import at \(.incremental.workers) workers imported \(.incremental.full_import_actions) actions in \(.incremental.full_import_ms) ms and mirrored \(.incremental.disk_mirror_files) files (\(.incremental.disk_mirror_bytes) bytes) on disk; holds \(.incremental.holds)."' "$file"
    done
    echo
    echo "Throughput against the workers (the full import from an empty cache, best of alternating rounds after a warm-up; the idle share is the time the workers spent waiting for a ready action over their wall time):"
    echo
    echo "| Cell | Workers | Best ms | Assets per second | Speedup over 1 worker | Idle share | Rounds ms |"
    echo "| --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .throughput.per_worker_count[] | "| \($cell) | \(.workers) | \(.best_ms) | \(.assets_per_second) | \(.speedup) | \(.idle_share) | \([.rounds[].wall_ms] | map(tostring) | join(", ")) |"' "$file"
    done
    echo
    for file in "${facts[@]}"; do
      jq -r '"- `\(.cell)`: \(.throughput.rounds) rounds, warm-up \(.throughput.warm_up_ms) ms, every timed import gave the identity scenario'"'"'s manifests \(.throughput.every_manifest_identical). Refuting thresholds: below 1.6 at 4 workers on the x86_64 runner cells, below 3.0 at 4 workers on the ARM64 runner cells, below 10 at 16 workers on the founder'"'"'s machine."' "$file"
    done
    echo
    echo "The claiming order is not the output order (runs at the same worker count must complete in different sequences while the manifest stays the same):"
    echo
    for file in "${facts[@]}"; do
      jq -r '"- `\(.cell)`: \(.claiming_order.runs_needed) run(s) at \(.claiming_order.workers) workers, completion sequence hashes \([.claiming_order.runs[].completion_order_hash] | join(", ")); the sequences differ \(.claiming_order.orders_differ); the manifests are identical \(.claiming_order.manifests_identical); holds \(.claiming_order.holds)."' "$file"
    done
    ;;
  workload-scene-streaming)
    echo "Determinism per scenario: the scripted runs at every worker count against one another and the oracle of their log, the live run against the oracle of its log, the replays of the live log against the live run:"
    echo
    echo "| Cell | Scenario | Ticks | Scripted identical across workers | Scripted match the oracle | Scripted run hash | Live matches the oracle | Replays identical to live | Replays match the oracle | Loads | Unloads | Max loaded cells | Max remap entries | Holds |"
    echo "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .determinism.scenarios[] | "| \($cell) | \(.scenario) | \(.ticks) | \(.scripted_identical_across_workers) | \(.scripted_match_oracle) | \(.scripted_run_hash) | \(.live_matches_oracle) | \(.replays_identical_to_live) | \(.replays_match_oracle) | \(.loads) | \(.unloads) | \(.max_loaded_cells) | \(.max_remap_entries) | \(.holds) |"' "$file"
    done
    echo
    echo "Distinct scripted run hashes per scenario across every cell (1 means identical everywhere):"
    echo
    for scenario in $(jq -r '.determinism.scenarios[].scenario' "${facts[@]}" | sort -u); do
      distinct="$(jq -r --arg scenario "$scenario" '.determinism.scenarios[] | select(.scenario == $scenario) | .scripted_run_hash' "${facts[@]}" | sort -u | wc -l | tr -d ' ')"
      echo "- \`$scenario\`: $distinct distinct run hash value(s) across ${#facts[@]} cell(s)."
    done
    echo
    echo "Every run (the live and replayed runs of a scenario share the live log; the remap cost rounds, the hysteresis control, the unsliced control and the two worlds follow the matrix):"
    echo
    echo "| Cell | Scenario | Loader | Workers | Ticks | Final hash | Matches the oracle | First divergent tick | Reads absent | Reads present | Stale slots | Loads | Unloads | Abandoned arrivals | Max remap entries | Wall ms | Ticks per second | Mean tick us | Spawn us per cell | Despawn us per cell | Changes share of the tick | Peak changes us | Peak tick us |"
    echo "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .runs[] | "| \($cell) | \(.scenario) | \(.loader) | \(.workers) | \(.ticks) | \(.final_hash) | \(.matches_oracle) | \(.first_divergent_tick) | \(.reads_absent) | \(.reads_present) | \(.stale_slots) | \(.loads) | \(.unloads) | \(.abandoned_arrivals) | \(.max_remap_entries) | \(.wall_ms) | \(.ticks_per_second) | \(.mean_tick_us) | \(.spawn_us_per_cell) | \(.despawn_us_per_cell) | \(.changes_share_of_tick) | \(.peak_changes_us) | \(.peak_tick_us) |"' "$file"
    done
    echo
    echo "The verdicts, one line per hypothesis and cell:"
    echo
    for file in "${facts[@]}"; do
      jq -r '"- `\(.cell)` determinism: holds \(.determinism.holds) (\([.determinism.scenarios[] | select(.holds)] | length) of \(.determinism.scenarios | length) scenarios; \(.platform.available_parallelism) CPUs, profile \(.profile), worker counts \([.settings.worker_counts[].workers] | map(tostring) | join(", "))); replay mismatches \([.determinism.scenarios[].replay_mismatches] | add)."' "$file"
      jq -r '"- `\(.cell)` not present: holds \(.not_present.holds); reads while absent \(.not_present.reads_absent), reads while present \(.not_present.reads_present), stale slots \(.not_present.stale_slots), oracle check failures \(.not_present.oracle_check_failures), runs whose counts equal the oracle'"'"'s \(.not_present.runs_agreeing_with_oracle_counts) of \(.not_present.runs)."' "$file"
      jq -r '"- `\(.cell)` remap cost: measured \(.remap_cost.measured), holds \(.remap_cost.holds) (limit \(.remap_cost.difference_limit)); at \(.remap_cost.density) entities per cell, one cell per \(.remap_cost.ticks_per_cell) ticks, \(.remap_cost.workers) workers, \(.settings.rounds) rounds, the best round'"'"'s median per cell (the best mean in parentheses): \([.remap_cost.grids[] | "\(.cells) cells: spawn \(.best_spawn_us_per_cell) (\(.best_mean_spawn_us_per_cell)) us per cell, despawn \(.best_despawn_us_per_cell) (\(.best_mean_despawn_us_per_cell)) us per cell, table at most \(.max_remap_entries) entries (\(.max_loaded_cells) cells), \(.loads) loads, every round matches \(.every_round_matches_oracle)"] | join("; ")); differences spawn \(.remap_cost.spawn_difference), despawn \(.remap_cost.despawn_difference), table \(.remap_cost.table_difference)."' "$file"
      jq -r '"- `\(.cell)` hysteresis: holds \(.hysteresis.holds) (saving limit \(.hysteresis.saving_limit)); \(.hysteresis.scenario): \(.hysteresis.loads_with) loads and \(.hysteresis.unloads_with) unloads with the leaving distance one cell farther, \(.hysteresis.loads_without) loads and \(.hysteresis.unloads_without) unloads with equal distances, saving \(.hysteresis.saving); both match their oracle \(.hysteresis.with_matches_oracle and .hysteresis.without_matches_oracle)."' "$file"
      jq -r '"- `\(.cell)` cost: holds \(.cost.holds) (share limit \(.cost.share_limit)); \(.cost.sliced.scenario) at \(.cost.sliced.workers) workers: the structural changes take \(.cost.sliced.changes_share_of_tick) of the tick (the whole structural node \(.cost.sliced.structural_node_share_of_tick)), peak changes \(.cost.sliced.peak_changes_us) us, peak tick \(.cost.sliced.peak_tick_us) us, mean tick \(.cost.sliced.mean_tick_us) us, \(.cost.sliced.us_per_spawned_entity) us per spawned entity, \(.cost.sliced.ticks_per_second) ticks per second; unsliced: share \(.cost.unsliced.changes_share_of_tick), peak changes \(.cost.unsliced.peak_changes_us) us, peak tick \(.cost.unsliced.peak_tick_us) us, mean tick \(.cost.unsliced.mean_tick_us) us, \(.cost.unsliced.us_per_spawned_entity) us per spawned entity; both match their oracle \(.cost.sliced.matches_oracle and .cost.unsliced.matches_oracle)."' "$file"
      jq -r '"- `\(.cell)` two worlds: holds \(.two_worlds.holds); \(.two_worlds.first_scenario) and \(.two_worlds.second_scenario) at \(.two_worlds.workers) workers on one store and one loader: the first matches its oracle \(.two_worlds.first_matches_oracle) (\(.two_worlds.first_loads) loads), the second \(.two_worlds.second_matches_oracle) (\(.two_worlds.second_loads) loads), \(.two_worlds.payloads_in_store) payloads in the store for \(.two_worlds.imports) imports and \(.two_worlds.requests_served) requests served."' "$file"
    done
    ;;
  workload-nested-overrides)
    echo "Every world run: the three places the overrides of an instance are applied, at every worker count, against the sequential oracle of the same log:"
    echo
    echo "| Cell | Size | Application | Workers | State run hash | Content run hash | Matches the oracle | First divergent tick | Reads absent | Reads present | Stale slots | Instances spawned | Instances failed | Entities spawned | Slots | Gate polls | Ticks with a required instance missing | Spawns not prepared | Ticks per second | Spawn us per entity |"
    echo "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .scenarios[] | .size as $size | .runs[] | "| \($cell) | \($size) | \(.application) | \(.workers) | \(.state_run_hash) | \(.content_run_hash) | \(.matches_oracle) | \(.first_divergent_tick) | \(.reads_absent) | \(.reads_present) | \(.stale_slots) | \(.instances_spawned) | \(.instances_failed) | \(.entities_spawned) | \(.slots) | \(.gate_polls) | \(.ticks_with_a_required_instance_missing) | \(.spawns_not_prepared) | \(.ticks_per_second) | \(.spawn_us_per_entity) |"' "$file" | tr -d '\r'
    done
    echo
    echo "Distinct values across every cell and worker count (1 means identical everywhere):"
    echo
    for size in $(jq -r '.scenarios[].size' "${facts[@]}" | tr -d '\r' | sort -u); do
      distinct="$(jq -r --arg size "$size" '.scenarios[] | select(.size == $size) | .import.payloads_digest' "${facts[@]}" | tr -d '\r' | sort -u | wc -l | tr -d ' ')"
      echo "- \`$size\` imported payloads: $distinct distinct digest(s) across ${#facts[@]} cell(s)."
      for application in $(jq -r '.scenarios[].runs[].application' "${facts[@]}" | tr -d '\r' | sort -u); do
        state="$(jq -r --arg size "$size" --arg application "$application" '.scenarios[] | select(.size == $size) | .runs[] | select(.application == $application) | .state_run_hash' "${facts[@]}" | tr -d '\r' | sort -u | wc -l | tr -d ' ')"
        content="$(jq -r --arg size "$size" --arg application "$application" '.scenarios[] | select(.size == $size) | .runs[] | select(.application == $application) | .content_run_hash' "${facts[@]}" | tr -d '\r' | sort -u | wc -l | tr -d ' ')"
        echo "- \`$size\` \`$application\`: $state distinct state run hash value(s), $content distinct content run hash value(s)."
      done
    done
    echo
    echo "How each scripted family appeared (the run of the prepared payload at the first worker count):"
    echo
    echo "| Cell | Size | Family | Required | Command tick | Depth | Instances | Entities | Children failed | First tick | Last tick | Distinct ticks | Parents first | Holds |"
    echo "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .scenarios[] | .size as $size | .families[] | "| \($cell) | \($size) | \(.family) | \(.required) | \(.command_tick) | \(.depth) | \(.instances) | \(.entities) | \(.children_failed) | \(.first_tick) | \(.last_tick) | \(.distinct_ticks) | \(.parents_first) | \(.holds) |"' "$file" | tr -d '\r'
    done
    echo
    echo "The verdicts, per cell and size:"
    echo
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .platform.available_parallelism as $cpus | .profile as $profile | .settings.worker_counts as $workers | .scenarios[] | "- `\($cell)` `\(.size)` (\($cpus) CPUs, profile \($profile), worker counts \($workers)): determinism holds \(.determinism.holds) (\(.determinism.runs_matching_their_oracle) of \(.determinism.world_runs) runs match their oracle, \(.determinism.stale_slots) stale slots); import holds \(.import.holds) (\(.import.lists) lists, \(.import.overrides_on_the_child + .import.overrides_through_a_path) overrides of which \(.import.overrides_through_a_path) through a path and \(.import.outer_overrides_on_what_an_inner_list_set) on what an inner list set; named conflicts refused by name \(.import.named_conflicts_refused_by_name) of \(.import.named_conflicts), all planted \(.import.planted_conflicts_refused_by_name) of \(.import.planted_conflicts), accepted \(.import.planted_conflicts_accepted); \(.import.orders_of_application_tried) orders of application, \(.import.orders_of_application_differing) differing); payload equals ordered in staging \(.application_paths.payload_equals_ordered_in_staging); ordered in the world spawns the same content \(.application_paths.ordered_in_world_spawns_the_same_content), same state hash \(.application_paths.ordered_in_world_same_state_hash) (first other at tick \(.application_paths.ordered_in_world_first_tick_of_another_state_hash)), same content hash \(.application_paths.ordered_in_world_same_content_hash) (first other at tick \(.application_paths.ordered_in_world_first_tick_of_another_content_hash)); \(.application_paths.prepared_payloads) prepared payloads for \(.application_paths.prepared_instances) instances; families hold \(.families_hold); another hash fails \(.another_hash.holds) (\(.another_hash.children_failed) of \(.another_hash.children_expected_to_fail) children failed, \(.another_hash.children_spawned_at_another_hash) spawned); identities hold \(.identities.holds) (\(.identities.by_application[0].kept) of \(.identities.by_application[0].entities) full references kept at the checkpoints, \(.identities.by_application[0].overridden) of them overridden, \([.identities.by_application[].changed] | add) changed; against the world without overrides: \(.identities.against_the_world_without_overrides.full_references_kept) kept of which \(.identities.against_the_world_without_overrides.kept_and_overridden) overridden, \(.identities.against_the_world_without_overrides.removed_by_an_override) removed, \(.identities.against_the_world_without_overrides.inside_a_removed_instance) inside a removed instance, \(.identities.against_the_world_without_overrides.added_by_an_override) added, \(.identities.against_the_world_without_overrides.unexplained) unexplained)."' "$file" | tr -d '\r'
    done
    ;;
  box2d-pool-sharing)
    echo "| Cell | Scene | Scheduler | Team | Library workers | Final hash | Matches the worker sweep | First divergent step | Step ms total | Cost over native (same team) | Cost over native (same library workers) | Worker parks | Finish parks | Synthetic chunks per step |"
    echo "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .runs[] | "| \($cell) | \(.scene) | \(.scheduler) | \(.team_workers) | \(.library_workers) | \(.final_hash) | \(.matches_worker_sweep) | \(.first_divergent_step) | \(.step_ms_total) | \(.cost_over_native) | \(.cost_over_native_same_library_workers) | \(.worker_parks) | \(.finish_parks) | \(.synthetic_chunks_per_step) |"' "$file"
    done
    echo
    echo "Distinct final hashes per scene across every cell, scheduler and team (1 means identical everywhere):"
    echo
    for scene in $(jq -r '.runs[].scene' "${facts[@]}" | sort -u); do
      distinct="$(jq -r --arg scene "$scene" '.runs[] | select(.scene == $scene) | .final_hash' "${facts[@]}" | sort -u | wc -l | tr -d ' ')"
      echo "- \`$scene\`: $distinct distinct final hash value(s)."
    done
    echo
    echo "Per cell: runs that match the worker sweep's hashes, whole-team runs within the 10 percent threshold over the native scheduler, and the worst costs:"
    echo
    for file in "${facts[@]}"; do
      jq -r '"- `\(.cell)` (\(.platform.cores) logical processors, quick \(.quick)): \([.runs[] | select(.matches_worker_sweep)] | length) of \(.runs | length) runs match the worker sweep; whole team within 1.10 of native: \([.runs[] | select(.scheduler == "engine_whole_team" and .cost_over_native <= 1.10)] | length) of \([.runs[] | select(.scheduler == "engine_whole_team")] | length) runs, worst \([.runs[] | select(.scheduler == "engine_whole_team") | .cost_over_native] | max) (\([.runs[] | select(.scheduler == "engine_whole_team")] | max_by(.cost_over_native) | "\(.scene) at \(.team_workers) workers")); dedicated subset over native at the same library worker count: worst \([.runs[] | select(.scheduler == "engine_dedicated_subset") | .cost_over_native_same_library_workers] | max)."' "$file"
    done
    ;;
  rapier-snapshot-continuation)
    echo "| Cell | Dimension | Scene | Pool threads | Bodies hashed | Snapshot bytes | Bytes per body | Serialize ms | Second world bytes identical | Path | Path threads | Restore succeeded | Hash at restore matches | First divergent step | Restore ms |"
    echo "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .runs[] | . as $run | .paths[] | "| \($cell) | \($run.dimension) | \($run.scene) | \($run.pool_threads) | \($run.hashed_body_count) of \($run.body_count) | \($run.snapshot_bytes) | \($run.bytes_per_body) | \($run.serialize_ms) | \($run.second_world_bytes_identical) | \(.path) | \(.pool_threads) | \(.restore_succeeded) | \(.hash_at_restore_matches) | \(.first_divergent_step) | \(.restore_ms) |"' "$file"
    done
    echo
    echo "Paths that reproduce the uninterrupted run at every step after the snapshot, per cell:"
    echo
    for file in "${facts[@]}"; do
      jq -r '"- `\(.cell)` (parallel feature \(.rapier.parallel_feature)): \([.runs[].paths[] | select(.restore_succeeded and .hash_at_restore_matches and .first_divergent_step == -1)] | length) of \([.runs[].paths[]] | length) paths; second serializations identical in \([.runs[] | select(.second_world_bytes_identical)] | length) of \(.runs | length) runs."' "$file"
    done
    echo
    echo "Distinct uninterrupted final hashes per dimension, scene and pool size across every cell (1 means identical everywhere):"
    echo
    for config in $(jq -r '.runs[] | "\(.dimension)/\(.scene)/\(.pool_threads)"' "${facts[@]}" | sort -u); do
      distinct="$(jq -r --arg config "$config" '.runs[] | select("\(.dimension)/\(.scene)/\(.pool_threads)" == $config) | .uninterrupted_final_hash' "${facts[@]}" | sort -u | wc -l | tr -d ' ')"
      echo "- \`$config\`: $distinct distinct final hash value(s)."
    done
    echo
    echo "| Cell | Dimension | Scene | Mode | Threads | Final hash | Matches the baseline | Step ms total | Step ms median |"
    echo "| --- | --- | --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .pool_sharing[] | "| \($cell) | \(.dimension) | \(.scene) | \(.mode) | \(.threads) | \(.final_hash) | \(.matches_baseline) | \(.step_ms_total) | \(.step_ms_median) |"' "$file"
    done
    ;;
  jolt-snapshot-continuation)
    echo "| Cell | Scene | Threads | Bodies hashed | State bytes | Bytes per body | Save ms | Second world state identical | Update errors | Path | Path threads | Restore succeeded | Hash at restore matches | First divergent step | Restore ms |"
    echo "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .runs[] | . as $run | .paths[] | "| \($cell) | \($run.scene) | \($run.threads) | \($run.hashed_body_count) of \($run.body_count) | \($run.state_bytes) | \($run.bytes_per_body) | \($run.save_ms) | \($run.second_world_state_identical) | \($run.uninterrupted_update_errors) | \(.path) | \(.threads) | \(.restore_succeeded) | \(.hash_at_restore_matches) | \(.first_divergent_step) | \(.restore_ms) |"' "$file"
    done
    echo
    echo "Paths that reproduce the uninterrupted run at every step after the save, per cell:"
    echo
    for file in "${facts[@]}"; do
      jq -r '"- `\(.cell)` (\(.jolt.configuration)): \([.runs[].paths[] | select(.restore_succeeded and .hash_at_restore_matches and .first_divergent_step == -1)] | length) of \([.runs[].paths[]] | length) paths; second saves identical in \([.runs[] | select(.second_world_state_identical)] | length) of \(.runs | length) runs."' "$file"
    done
    echo
    echo "Distinct uninterrupted final hashes per scene and thread count across every cell (1 means identical everywhere):"
    echo
    for config in $(jq -r '.runs[] | "\(.scene)/\(.threads)"' "${facts[@]}" | sort -u); do
      distinct="$(jq -r --arg config "$config" '.runs[] | select("\(.scene)/\(.threads)" == $config) | .uninterrupted_final_hash' "${facts[@]}" | sort -u | wc -l | tr -d ' ')"
      echo "- \`$config\`: $distinct distinct final hash value(s)."
    done
    ;;
  control-surface-data-plane)
    echo "Verdicts per cell, computed by the spike:"
    echo
    echo "| Cell | Hypothesis | Verdict | Refuted by its threshold | Figure |"
    echo "| --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .hypotheses[] | "| \($cell) | \(.hypothesis) | \(.verdict) | \(.refuted_by_its_threshold) | \(.figure) |"' "$file"
    done
    echo
    echo "The ring's client cost against the full-state read through the socket, and the world's throughput (best of the repetitions, over the time spent stepping):"
    echo
    echo "| Cell | Workers | Ring client us per state frame | Socket read us (median) | Socket | Ratio | Direct ticks per second | Lossless ring loss percent | Extraction only loss percent | Stalled latest-wins loss percent |"
    echo "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .runs[] | "| \($cell) | \(.workers) | \(.ring_client.us_per_state_frame) | \(.ring_client.socket_read_median_us) | \(.ring_client.socket_read_transport) | \(.ring_client.ratio_to_the_socket_read) | \(.direct.ticks_per_second) | \(.ring_client.lossless_throughput_loss_percent) | \(.ring_client.extraction_only_throughput_loss_percent) | \(.ring_client.stalled_latest_wins_throughput_loss_percent) |"' "$file"
    done
    echo
    echo "Ways of stepping the world through a stepped session in batches of ten ticks (the first six repeated, the last two run once):"
    echo
    echo "| Cell | Workers | Way | Transport | Ticks per second | Ratio to direct | Agent ticks per second | Matches the oracle | First divergent tick | State frames | State ticks not delivered | Client us per state frame | Endpoint copy us per state frame | Extraction us per tick | Frames overwritten | Ticks skipped | A stream ended |"
    echo "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .runs[] | .workers as $w | (.throughput + .single_runs)[] | "| \($cell) | \($w) | \(.way) | \(.transport) | \(.ticks_per_second) | \(.ratio_to_direct) | \(.agent_ticks_per_second) | \(.matches_oracle) | \(.first_divergent_tick) | \(.state_frames) | \(.state_frames_missing) | \(.client_us_per_state_frame) | \(.endpoint_copy_us_per_state_frame) | \(.extraction_us_per_tick) | \(.frames_overwritten) | \(.ticks_skipped) | \(.a_stream_ended) |"' "$file"
    done
    echo
    echo "The full state read through the socket:"
    echo
    echo "| Cell | Workers | Transport | Reads | State bytes | Median us | Mean us | Client hash us |"
    echo "| --- | --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .runs[] | .workers as $w | .socket_reads[] | "| \($cell) | \($w) | \(.transport) | \(.reads) | \(.state_bytes) | \(.median_us) | \(.mean_us) | \(.client_hash_us) |"' "$file"
    done
    echo
    echo "Clients that stop reading a lossless stream, then resume by cursor (stepped sessions per worker count, and the paced session):"
    echo
    echo "| Cell | Workers | Session | Data plane | Stalled at | Ended at | Ended with | Resumed at | Ends | State frames | Missing | Duplicated | Every frame once | Content mismatches | Matches the oracle | Holds |"
    echo "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | ((.runs[] | .workers as $w | .stalled_lossless_clients[] | . + {workers: $w, session: "stepped"}), (.paced + {session: "paced"})) | "| \($cell) | \(.workers) | \(.session) | \(.data_plane) | \(.client_stalled_at) | \(.streams_ended_at) | \(.ended_with) | \(.resumed_at) | \(.state_stream_ends) state, \(.frame_stream_ends) frame | \(.state_frames) | \(.state_frames_missing) | \(.state_frames_duplicated) | \(.every_frame_once) | \(.content_mismatches) | \(.matches_oracle) | \(.holds) |"' "$file"
    done
    echo
    echo "Transactions per size (five kinds each: field sets, component adds and removes, despawns, spawns, mixed):"
    echo
    echo "| Cell | Workers | Data plane | Size | Kinds atomic | One refused element: verdicts per kind | Refusals listed of planted under full evaluation | Inverse restores the full hash | Inverse restores the component data and labels | Original applies again |"
    echo "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .runs[] | .workers as $w | .commands[] | .data_plane.data_plane as $plane | (.transactions | group_by(.size))[] | "| \($cell) | \($w) | \($plane) | \(.[0].size) | \([.[] | select(.atomic)] | length) of \(length) | \([.[].one_refused_verdicts] | unique | join("; ")) | \([.[].refusals_listed] | add) of \([.[].refusals_planted] | add) | \([.[] | select(.inverse_restores_the_full_hash)] | length) of \(length) | \([.[] | select(.inverse_restores_the_component_data and .inverse_restores_the_labels)] | length) of \(length) | \([.[] | select(.original_applies_again)] | length) of \(length) |"' "$file"
    done
    echo
    echo "The kinds and sizes whose inverse does not restore the full hash, per cell:"
    echo
    for file in "${facts[@]}"; do
      jq -r '"- `\(.cell)`: \([.runs[].commands[].transactions[] | select(.inverse_restores_the_full_hash | not) | "\(.kind) of \(.size)"] | unique | if length == 0 then "none" else join(", ") end)."' "$file"
    done
    echo
    echo "The scripted agent's commands and the per-session sequence:"
    echo
    echo "| Cell | Workers | Data plane | Matches the oracle | First divergent tick | Commands sent | In the input log | Outcome mismatches | Digest mismatches | Every frame once | Retries answered alike | Gaps refused | Altered retries refused by name | Retry outside retention refused | Sequence holds |"
    echo "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .runs[] | .workers as $w | .commands[] | "| \($cell) | \($w) | \(.data_plane.data_plane) | \(.matches_oracle) | \(.first_divergent_tick) | \(.commands_sent) | \(.commands_in_the_input_log) | \(.outcome_mismatches) | \(.digest_mismatches) | \(.data_plane.every_frame_once) | \(.sequence.retries_answered_alike) of \(.sequence.retries_sent) | \(.sequence.gaps_refused) of \(.sequence.gaps_sent) | \(.sequence.altered_retries_refused_by_name) of \(.sequence.altered_retries_sent) | \(.sequence.retry_outside_retention_refused) | \(.sequence.holds) |"' "$file"
    done
    echo
    echo "The paced session (the host steps one tick per millisecond; every command's result is asked for by retrying its sequence):"
    echo
    for file in "${facts[@]}"; do
      jq -r '"- `\(.cell)`: \(.paced.workers) workers, \(.paced.host_ticks_per_second) host ticks per second, matches the oracle \(.paced.matches_oracle), \(.paced.commands_sent) commands sent and \(.paced.commands_in_the_input_log) in the input log, \(.paced.results_answered_from_the_log) results answered from the log, \(.paced.commands_never_applied) never applied."' "$file"
    done
    echo
    echo "The inverse's cost, in process (median nanoseconds per transaction):"
    echo
    echo "| Cell | Kind | Size | Apply ns | Apply with inverse ns | Inverse percent of apply |"
    echo "| --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .inverse_cost[] | "| \($cell) | \(.kind) | \(.size) | \(.apply_ns) | \(.apply_with_inverse_ns) | \(.inverse_percent_of_apply) |"' "$file"
    done
    ;;
  test-recording-round-trip)
    echo "| Cell | Profile | Every check holds | Complete run (passed, failed) | Ended run (outcome, interrupted, not run) | Classes agree with what the tests said | Resumed run equals the complete run | Recordings carried from other runs | Every recording verified |"
    echo "| --- | --- | --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '"| \(.cell) | \(.profile) | \(.every_check_holds) | \(.complete_run.passed), \(.complete_run.failed) | \(.ended_run.with_an_outcome), \(.ended_run.interrupted), \(.ended_run.not_run) | \(.ended_run.classes_agree_with_what_the_tests_said) | \(.resumed_run.same_outcomes_and_hashes_as_the_complete_run) | \(.recordings_carried_from_other_runs) | \(.every_recording_verified) |"' "$file"
    done
    echo
    echo "Every recording a cell listed and replayed (the verdict per worker count):"
    echo
    echo "| Reading cell | File | Written by | Form | Bytes | Last tick | Last recorded hash | Listed from the description | Verdicts |"
    echo "| --- | --- | --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | (.recordings + .measured_size_recording.files)[] | "| \($cell) | \(.file) | \(.writer_cell) (\(.writer_profile)) | \(.form) | \(.bytes) | \(.end_tick) | \(.last_recorded_hash) | \(.listed_from_the_description) | \([.replays[] | "\(.workers): \(.verdict)"] | join("; ")) |"' "$file"
    done
    echo
    echo "Distinct digests across cells of the sections that hold no writer's name, per file kind:"
    echo
    for kind in self-contained reference; do
      for tag in snap prep inpt hash; do
        distinct="$(jq -r --arg kind "$kind" --arg tag "$tag" '.recordings[] | select(.origin | startswith("this run")) | select(.form == $kind) | .sections[]? | select(.tag == $tag) | .digest' "${facts[@]}" | sort -u | wc -l | tr -d ' ')"
        if [ "$distinct" -gt 0 ]; then
          echo "- \`$kind\`, section \`$tag\`: $distinct distinct digest(s) across ${#facts[@]} cell(s)."
        fi
      done
    done
    for tag in snap prep inpt hash; do
      distinct="$(jq -r --arg tag "$tag" '.measured_size_recording.files[] | .sections[]? | select(.tag == $tag) | .digest' "${facts[@]}" | sort -u | wc -l | tr -d ' ')"
      echo "- the measured size's files, section \`$tag\`: $distinct distinct digest(s) across ${#facts[@]} cell(s)."
    done
    echo
    echo "Changed and cut files:"
    echo
    echo "| Cell | One byte changed in place | One byte changed, file made whole: changed tick, reported tick | Preparation records changed: refused with no record applied, same starting state, other | Cut files refused before any step | Hashes trimmed |"
    echo "| --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '"| \(.cell) | \(.damage.one_byte_changed_in_place.verdict): \(.damage.one_byte_changed_in_place.reason) | \(.damage.one_byte_changed_and_the_file_made_whole.changed_record_tick), \(.damage.one_byte_changed_and_the_file_made_whole.first_divergent_tick) | \(.damage.one_preparation_record_changed.refused_at_the_starting_hash_with_no_record_applied), \(.damage.one_preparation_record_changed.same_starting_state_and_verified), \(.damage.one_preparation_record_changed.any_other_outcome) | \([.damage.cut_files[] | select(.refused_before_any_step)] | length) of \(.damage.cut_files | length) | \(.damage.hashes_trimmed.verdict) |"' "$file"
    done
    echo
    echo "The cost of an invariant over every entity read after each one-tick step (microseconds per tick, best round):"
    echo
    echo "| Cell | Size | Workers | Hash cadence | One multi-tick step | One-tick steps | One-tick steps with the invariant | Invariant | With the invariant over one step | Invariant alone ns |"
    echo "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .invariant_cost as $c | $c.rows[] | "| \($cell) | \($c.size) | \(.workers) | \(.hash_cadence) | \(.one_multi_tick_step_us_per_tick) | \(.one_tick_steps_us_per_tick) | \(.one_tick_steps_with_the_invariant_us_per_tick) | \(.invariant_us_per_tick) | \(.one_tick_steps_with_the_invariant_over_one_step) | \($c.invariant_alone_ns) |"' "$file"
    done
    ;;
  input-records-from-device-events)
    echo "| Cell | Profile | Every check holds | Record bytes (8-byte epoch) | Mean bytes changed per record | Mapping ns per tick | Applying ns per tick |"
    echo "| --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '"| \(.cell) | \(.profile) | \(.every_check_holds) | \(.record_size.bytes_with_an_epoch_of_8_bytes) | \(.record_size.mean_bytes_that_differ_from_the_previous_record) | \(.cost_per_tick_best_of_five_rounds.mapping_the_events_and_taking_the_record_ns) | \(.cost_per_tick_best_of_five_rounds.applying_the_record_ns) |"' "$file"
    done
    echo
    echo "Presses against frames and ticks (the tick of application minus the tick of the press: earliest, latest, mean):"
    echo
    echo "| Cell | Schedule | Event timestamps | Ticks | Frames without a tick | Most ticks in a frame | Totals equal the stream | Earliest | Latest | Mean |"
    echo "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .frames_and_ticks[] | "| \($cell) | \(.schedule) | \(.event_timestamps) | \(.ticks) | \(.frames_without_a_tick) | \(.most_ticks_in_a_frame) | \(.presses_releases_and_motion_equal_the_stream) | \(.earliest_tick_of_application_against_the_tick_of_the_press) | \(.latest_tick_of_application_against_the_tick_of_the_press) | \(.mean_ticks_between_the_press_and_its_application) |"' "$file"
    done
    echo
    echo "A restore under a live writer, with nothing done at the restore:"
    echo
    for file in "${facts[@]}"; do
      jq -r '"- `\(.cell)`: the next record is \(.restore_under_a_live_writer.with_nothing_done_the_next_record_is) and applies \(.restore_under_a_live_writer.presses_that_record_applies) presses where the writer counted \(.restore_under_a_live_writer.presses_the_writer_counted_in_that_tick)."' "$file"
    done
    echo
    echo "The device layer on each cell (threads before, while running or existing, after the drop):"
    echo
    echo "| Cell | Probe | Gamepad library | Gamepads | Threads | Windowing library | Threads | Display variables set (X11, Wayland) |"
    echo "| --- | --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.device_layer as $d | ($d.report // {}) as $r | "| \(.cell) | \($d.outcome) | \($r.gamepad_library.outcome // "n/a")\(if $r.gamepad_library.error then ": " + $r.gamepad_library.error else "" end) | \($r.gamepad_library.gamepads_listed // "n/a") | \($r.gamepad_library.threads_before // "n/a"), \($r.gamepad_library.threads_while_running // $r.gamepad_library.threads_after_the_failure // "n/a"), \($r.gamepad_library.threads_after_the_drop // "n/a") | \($r.windowing_library.outcome // "n/a")\(if $r.windowing_library.error then ": " + $r.windowing_library.error else "" end) | \($r.windowing_library.threads_before // "n/a"), \($r.windowing_library.threads_while_it_exists // $r.windowing_library.threads_after_the_failure // "n/a"), \($r.windowing_library.threads_after_the_drop // "n/a") | \($r.display_variable_set | if . == null then "n/a" else . end), \($r.wayland_display_variable_set | if . == null then "n/a" else . end) |"' "$file"
    done
    ;;
  audio-forms-and-threads)
    echo "| Cell | Profile | Every check holds | Triangle samples hash | Platform sine samples hash | Mixer ns per buffer of 512 frames, 64 voices |"
    echo "| --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '"| \(.cell) | \(.profile) | \(.every_check_holds) | \(.offline_samples.triangle_in_exact_arithmetic.samples_hash) | \(.offline_samples.the_platform_sine.samples_hash) | \(.mixer_cost.best_ns_per_buffer) |"' "$file"
    done
    echo
    for wave in triangle_in_exact_arithmetic the_platform_sine; do
      distinct="$(jq -r ".offline_samples.$wave.samples_hash" "${facts[@]}" | sort -u | wc -l | tr -d ' ')"
      echo "- \`$wave\`: $distinct distinct samples hash(es) across ${#facts[@]} cell(s)."
    done
    echo
    echo "The offline count (frames per tick: fewest, most; total over 600 ticks):"
    echo
    echo "| Cell | Rate | Step | Fewest | Most | Total | Total is the floor | Position follows the tick | Resumed run agrees |"
    echo "| --- | --- | --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .offline_count[] | "| \($cell) | \(.rate) | \(.step) | \(.fewest_frames_in_a_tick) | \(.most_frames_in_a_tick) | \(.frames_over_the_run) | \(.the_total_is_the_floor_of_rate_times_elapsed_time) | \(.the_mixer_position_after_each_tick_follows_the_tick_index) | \(.a_run_resumed_at_tick_300_mixes_the_same_frames_per_tick) |"' "$file"
    done
    echo
    echo "The null form: what a process that never calls the output library has loaded."
    echo
    for file in "${facts[@]}"; do
      jq -r '"- `\(.cell)`: \(.null_form.process); audio modules loaded: \((.null_form.report.loaded_audio_modules // []) | if length == 0 then "none" else join(", ") end); \(.null_form.report.loaded_modules // "n/a") modules in all."' "$file"
    done
    echo
    echo "The device form (threads: before the library, with the host, with the stream built, running, after the stream is dropped, after the host is dropped):"
    echo
    echo "| Cell | Probe | Host | Outcome | Format, rate, channels | Threads | Callbacks | Frames per callback (fewest, most) | Playback lead ms | Callback thread policy, priority | Mixer allocations | Hashes equal the reference | Errors |"
    echo "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.device_form as $d | ($d.report // {}) as $r | "| \(.cell) | \($d.process) | \($r.host // "n/a") | \($r.outcome // "n/a")\(if $r.error then ": " + $r.error else "" end) | \($r.default_sample_format // "n/a"), \($r.default_rate // "n/a"), \($r.default_channels // "n/a") | \($r.threads_before_the_library // "n/a"), \($r.threads_with_the_host // "n/a"), \($r.threads_with_the_stream_built // "n/a"), \($r.threads_with_the_stream_running // "n/a"), \($r.threads_after_the_stream_is_dropped // "n/a"), \($r.threads_after_the_host_is_dropped // "n/a") | \($r.callbacks // "n/a") | \($r.fewest_frames_in_a_callback // "n/a"), \($r.most_frames_in_a_callback // "n/a") | \($r.playback_lead_ms_at_the_last_callback // "n/a") | \($r.callback_thread_scheduling_policy // "n/a"), \($r.callback_thread_priority // "n/a") | \($r.allocations_in_the_mixer // "n/a") | \($r.the_device_form_gives_the_reference_hashes | if . == null then "n/a" else . end) | \(($r.errors_reported_by_the_library // []) | join("; ")) |"' "$file"
    done
    echo
    for file in "${facts[@]}"; do
      jq -r '"- `\(.cell)`: thread names with the stream running: \((.device_form.report.thread_names_with_the_stream_running // []) | join(", ")); audio modules: \((.device_form.report.audio_modules_with_the_stream_running // .device_form.report.audio_modules_after_the_lookup // .device_form.report.audio_modules_after_the_failure // []) | join(", "))."' "$file"
    done
    ;;
  workload-asset-table)
    echo "The verdict of each hypothesis per cell (the cost of a handle is a magnitude, judged on one quiet machine; the others are identities):"
    echo
    echo "| Cell | Profile | Size | Ticks | CPUs | A handle's cost | Wants | The gate during a run | A revision as one batch | The table under the hash | A save read by another process | One resident copy |"
    echo "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.hypotheses as $h | "| \(.cell) | \(.profile) | \(.size) | \(.ticks) | \(.platform.available_parallelism) | \($h.a_handles_cost) | \($h.wants) | \($h.the_gate_during_a_run) | \($h.a_revision_as_one_batch) | \($h.the_table_under_the_hash) | \($h.a_save_read_by_another_process) | \($h.one_resident_copy) |"' "$file"
    done
    echo
    for file in "${facts[@]}"; do
      jq -r '"- `\(.cell)`: reference run hash \(.reference_run_hash); the oracle with every leaf at the specification equals the reference \(.oracle_with_every_leaf_at_the_specification_equals_reference); the sequential reference runs \(.reference_ticks_per_second) ticks per second."' "$file"
    done
    echo
    echo "A handle's cost: every entity names one of the distinct assets, resolved in the table once per chunk and distinct asset, against the world that reads one runtime value and has no asset (ratios of ticks per second to that world, in alternating rounds; the generational index form is recorded beside):"
    echo
    echo "| Cell | Workers | Form | Distinct assets | Rounds | Best | Median | Ratio of bests | Ratio of medians | Resolutions per tick | Allocations in the timed step | Every run matches the reference |"
    echo "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .handle_cost[] | "| \($cell) | \(.workers) | \(.form) | \(.distinct_assets) | \(.rounds) | \(.best_ticks_per_second) | \(.median_ticks_per_second) | \(.ratio_of_bests) | \(.ratio_of_medians) | \(.resolutions_per_tick) | \(.allocations_in_the_timed_step) | \(.every_run_matches_reference) |"' "$file"
    done
    echo
    echo "Wants (scripted: two entities want one asset and are despawned one after the other by the workload; a root is wanted, answered with its closure by one batch, then released; an arrival names an asset no want names; an asset is wanted again after it fell):"
    echo
    echo "| Cell | Workers | State run hash | First divergent tick from the oracle | Replayed at | Record ticks | Log bytes | Checks that fail |"
    echo "| --- | --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .wants.runs[] | "| \($cell) | \(.workers) | \(.state_run_hash) | \(.first_divergent_tick_from_oracle) | \(.replay_workers) workers | \(.record_ticks) | \(.log_bytes) | \([to_entries[] | select(.value == false) | .key] | if length == 0 then "none" else join(", ") end) |"' "$file"
    done
    echo
    for file in "${facts[@]}"; do
      jq -r '"- `\(.cell)`: the owners are the entities of slots \(.wants.first_owner_slot) and \(.wants.second_owner_slot), despawned at ticks \(.wants.first_owner_despawned_at) and \(.wants.second_owner_despawned_at); the table after a fifth of the run is \(.wants.table_bytes_after_a_fifth_of_the_run) bytes, hash \(.wants.table_hash_after_a_fifth_of_the_run)."' "$file"
    done
    distinct="$(jq -r '.profile + " " + .size + " " + (.wants.runs[].state_run_hash)' "${facts[@]}" | sort -u | wc -l | tr -d ' ')"
    echo "- Distinct state run hashes of the wants scenario across every cell and worker count, per profile and size (1 per profile and size means identical everywhere): $distinct."
    distinct="$(jq -r '.profile + " " + .size + " " + .wants.table_hash_after_a_fifth_of_the_run' "${facts[@]}" | sort -u | wc -l | tr -d ' ')"
    echo "- Distinct hashes of the table's canonical bytes after a fifth of the run, per profile and size: $distinct."
    echo
    echo "The gate during a run (a live loader with a real delay: the composition's required set before the first tick, an instance made from a root with a closure of three levels, an instance made from a corrupt root, a required want nothing answers and the host releases):"
    echo
    echo "| Cell | Workers | Loader delay | Loader delays ms | Step while loading | Gates (before tick: unanswered, waited ms) | Step without waiting | Step with a deadline, waited ms | Record ticks | State run hash | First divergent tick from the oracle | Checks that fail |"
    echo "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .gate.runs[] | "| \($cell) | \(.workers) | \(.delay) | \(.loader_delays_ms) | \(.step_while_loading) | \([.gates[] | "\(.before_tick): \(.unanswered), \(.waited_ms)"] | join("; ")) | \(.step_without_waiting.ticks_ran) ticks, gated \(.step_without_waiting.gated), \(.step_without_waiting.unanswered) | \(.step_with_a_deadline.ticks_ran) ticks, gated \(.step_with_a_deadline.gated), \(.deadline_waited_ms) | \(.record_ticks) | \(.state_run_hash) | \(.first_divergent_tick_from_oracle) | \([to_entries[] | select(.value == false) | .key] | if length == 0 then "none" else join(", ") end) |"' "$file"
    done
    echo
    for file in "${facts[@]}"; do
      jq -r '"- `\(.cell)`: \(.gate.distinct_state_run_hashes) distinct state run hash value(s) across its delays and worker counts; the failed required asset as the step names it: \([.gate.runs[].step_through_two_gates.failed_required] | unique | join(" / "))."' "$file"
    done
    distinct="$(jq -r '.profile + " " + .size + " " + (.gate.runs[].state_run_hash)' "${facts[@]}" | sort -u | wc -l | tr -d ' ')"
    echo "- Distinct state run hashes of the gate scenario across every cell, delay and worker count, per profile and size: $distinct."
    echo
    echo "A revision as one batch (eight payloads landing one by one in a random order while the world steps, posted once all are resident; then two revisions posted before one drain):"
    echo
    echo "| Cell | Workers | Spread ms | Payloads landed at ms | Landing order | Identifier order | Tick at the first payload | Applied at tick | Partial applications compared | Merged at tick | Record ticks | First divergent tick from the oracle | Checks that fail |"
    echo "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .revision | "| \($cell) | \(.workers) | \(.spread_ms) | \(.payloads_landed_at_ms) | \(.landing_order) | \(.identifier_order) | \(.tick_at_the_first_payload) | \(.applied_at_tick) | \(.partial_applications_compared) | \(.merged_at_tick) | \(.record_ticks) | \(.first_divergent_tick_from_oracle) | \([to_entries[] | select(.value == false) | .key] | if length == 0 then "none" else join(", ") end) |"' "$file"
    done
    echo
    echo "The table under the hash (two worlds equal in every component, different in the hash of one asset no entity names):"
    echo
    for file in "${facts[@]}"; do
      jq -r '.table_under_the_hash as $t | "- `\(.cell)`: components equal at every tick \($t.components_equal_at_every_tick); state hashes differ from the arrival \($t.state_hashes_differ_from_the_arrival); table of \($t.table_bytes) bytes, hashes \($t.table_hash_of_the_one) and \($t.table_hash_of_the_other)."' "$file"
    done
    distinct="$(jq -r '.table_under_the_hash | .table_hash_of_the_one + " " + .table_hash_of_the_other' "${facts[@]}" | sort -u | wc -l | tr -d ' ')"
    echo "- Distinct pairs of table hashes across every cell (1 means the canonical bytes are identical everywhere): $distinct."
    echo
    echo "A save read by another process (the world saved at half of the run, then read as a new run on its answers and to continue its history):"
    echo
    for file in "${facts[@]}"; do
      jq -r '"- `\(.cell)`: saved at tick \(.save.saved_at_tick), \(.save.save_bytes) bytes, table hash \(.save.table_hash_at_the_save); the run matches its oracle \(.save.matches_oracle); a reload during the run instantiates the instance again \(.save.a_reload_instantiates_again); state run hash \(.save.state_run_hash)."' "$file"
    done
    echo
    echo "| Cell | Reader | Another process | Refused | Refused for, at hash, reason | Assets awaited | First tick waits for every answer | Answers in the first record | Reload events on the answers | Failure events | Instantiations | Instance failed | Step names | Record unchanged | Confirmations in the record | Matches expected | First divergent tick | Holds |"
    echo "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .save.readers[].report | "| \($cell) | \(.mode) | \(if .in_another_process == null then "n/a" else .in_another_process end) | \(if .refused == null then "n/a" else .refused end) | \(if .refused then "\(.refused_for), \(.refused_at_hash), \(.reason)" else (.error // "") end) | \(.assets_awaited // "n/a") | \(.first_tick_waits_for_every_answer // "n/a") | \(.answers_in_the_first_record // "n/a") | \(.reload_events_on_the_answers // "n/a") | \(.failure_events_on_the_answers // "n/a") | \(.instantiations_of_the_instance // "n/a") | \(if .instance_failed == null then "n/a" else .instance_failed end) | \(.step.failed_required // "n/a") | \(if .record_unchanged == null then "n/a" else .record_unchanged end) | \(.confirmations_in_the_record // "n/a") | \(if .matches_expected == null then "n/a" else .matches_expected end) | \(.first_divergent_tick // "n/a") | \(.holds) |"' "$file"
    done
    echo
    distinct="$(jq -r '.profile + " " + .size + " " + .save.state_run_hash + " " + .save.table_hash_at_the_save' "${facts[@]}" | sort -u | wc -l | tr -d ' ')"
    echo "- Distinct state run hashes and table hashes of the saved run across every cell, per profile and size: $distinct."
    echo
    echo "One resident copy (the live blocks of the size of the large asset's canonical part, counted by the allocator, while the worlds of one process hold it):"
    echo
    echo "| Cell | Worlds | Part bytes | Artifact reads | Available midway | Copies midway | References midway | Available at the end | Copies at the end | References at the end | Peak copies | Copies with a deliberate second one | Distinct run hashes | Holds |"
    echo "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .one_resident_copy | "| \($cell) | \(.worlds) | \(.part_bytes) | \(.reads_of_the_artifact) | \(.worlds_holding_it_available_midway) | \(.copies_midway) | \(.references_held_by_worlds_midway) | \(.worlds_holding_it_available_at_the_end) | \(.copies_at_the_end) | \(.references_held_by_worlds_at_the_end) | \(.peak_copies) | \(.copies_with_a_deliberate_second_one) | \(.distinct_run_hashes) | \(.holds) |"' "$file"
    done
    ;;
  asset-import-graph)
    echo "The verdict of each hypothesis per cell (the costs of the ready set and of the change journal are magnitudes, judged on one quiet machine; the others are identities and counts):"
    echo
    echo "| Cell | Profile | CPUs | Sources | Assets | Source bytes | One domain set | The two-pass graph on the ready set | The cascade stops | The key is complete | The change journal | A revision is atomic | A package is a function of its inputs | A scene-sized source is imported |"
    echo "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.hypotheses as $h | "| \(.cell) | \(.profile) | \(.platform.available_parallelism) | \(.set.sources) | \(.set.assets) | \(.set.source_bytes) | \($h.one_domain_set) | \($h.the_two_pass_graph_on_the_ready_set) | \($h.the_cascade_stops_where_bytes_stop_changing) | \($h.the_key_is_complete) | \($h.the_change_journal) | \($h.a_revision_is_atomic) | \($h.a_package_is_a_function_of_its_inputs) | \($h.a_scene_sized_text_source_is_imported) |"' "$file"
    done
    echo
    echo "One domain set (the full import from an empty cache per worker count and target; the presentation encoder mixes the worker's index into its output):"
    echo
    echo "| Cell | Workers | Target | Domain manifest | Canonical actions run | Presentation actions run | Wall ms | Failures |"
    echo "| --- | --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .one_domain_set.imports[] | "| \($cell) | \(.workers) | \(.target) | \(.domain_manifest) | \(.canonical_actions_run) | \(.presentation_actions_run) | \(.wall_ms) | \(.failures) |"' "$file"
    done
    echo
    for file in "${facts[@]}"; do
      jq -r '.one_domain_set as $o | "- `\(.cell)`: \($o.distinct_domain_manifests) distinct domain manifest(s) across its imports; \($o.distinct_desktop_presentation_manifests) distinct desktop and \($o.distinct_mobile_presentation_manifests) distinct mobile presentation manifests; a presentation manifest shared by the two targets \($o.a_presentation_manifest_shared_by_the_two_targets); each canonical action once for two targets \($o.each_canonical_action_once_for_two_targets)."' "$file"
    done
    distinct="$(jq -r '.profile + " " + .domain_manifest' "${facts[@]}" | sort -u | wc -l | tr -d ' ')"
    echo "- Distinct domain manifest hashes across every cell, per profile (1 per profile means one domain set everywhere): $distinct."
    echo
    echo "The two-pass graph (imports that end with a request and are run again, against every dependency declared in advance):"
    echo
    echo "| Cell | Form | Workers | Canonical imports | Ended with a request | Actions run again | Work run twice ms | Wall ms | Waiting share | Same manifest |"
    echo "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .two_pass_graph.imports[] | "| \($cell) | \(.form) | \(.workers) | \(.canonical_imports) | \(.imports_that_ended_with_a_request) | \(.actions_run_again) | \(.work_run_twice_ms) | \(.wall_ms) | \(.waiting_share) | \(.same_manifest) |"' "$file"
    done
    echo
    echo "Many small independent actions on the central ready set (the waiting share is the share of the workers' time spent outside an action):"
    echo
    echo "| Cell | Workers | Actions | Microseconds per action | Wall ms | Waiting share |"
    echo "| --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .two_pass_graph.independent_actions[] | "| \($cell) | \(.workers) | \(.actions) | \(.microseconds_per_action) | \(.wall_ms) | \(.waiting_share) |"' "$file"
    done
    echo
    for file in "${facts[@]}"; do
      jq -r '"- `\(.cell)`: a planted cycle of two requests fails both actions naming it \(.two_pass_graph.a_cycle_fails_both_actions_naming_it): \(.two_pass_graph.cycle_failures | .[0:260])"' "$file"
    done
    echo
    echo "The cascade and the key (re-imports on one cache; describe, canonical and presentation actions expected and run):"
    echo
    echo "| Cell | Change | Expected | Ran | Exactly the expected actions | Wall ms |"
    echo "| --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | (.cascade.re_imports + .the_key_is_complete.re_imports)[] | "| \($cell) | \(.change) | \(.expected_describe_canonical_presentation) | \(.ran_describe_canonical_presentation) | \(.exactly_the_expected_actions) | \(.wall_ms) |"' "$file"
    done
    echo
    for file in "${facts[@]}"; do
      jq -r '.the_key_is_complete as $k | "- `\(.cell)`: the copy names its own siblings \($k.the_copy_names_its_own_siblings); its sprites have the original bytes \($k.the_copy_s_sprites_have_the_original_s_bytes); a duplicate identifier is refused naming both files \($k.a_duplicate_is_refused_naming_both_files) (\($k.the_refusal))."' "$file"
    done
    distinct="$(jq -r '.profile + " " + ([(.cascade.re_imports + .the_key_is_complete.re_imports)[].ran_describe_canonical_presentation] | join(" / "))' "${facts[@]}" | sort -u | wc -l | tr -d ' ')"
    echo "- Distinct lists of action counts across every cell, per profile: $distinct."
    echo
    echo "The change journal (the pass that finds nothing to do against the full import at the same worker count; phases are the scan, the graph, the import actions and the revision):"
    echo
    echo "| Cell | Workers | Full import ms | Journal, a metadata call per file: ms, ratio, phases | Journal, facts from the directory listing: ms, ratio, phases | No journal: ms, ratio, phases | Sources digested (journal, none) | Nothing ran | The rewrite in place is missed by the journal | Found by the hermetic pass |"
    echo "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .change_journal | "| \($cell) | \(.workers) | \(.full_import_ms) | \(.nothing_to_do_with_the_journal_ms), \(.ratio_with_the_journal), (\(.scan_graph_imports_revision_ms_with_the_journal)) | \(.nothing_to_do_with_the_journal_and_the_listing_ms), \(.ratio_with_the_journal_and_the_listing), (\(.scan_graph_imports_revision_ms_with_the_journal_and_the_listing)) | \(.nothing_to_do_without_the_journal_ms), \(.ratio_without_the_journal), (\(.scan_graph_imports_revision_ms_without_the_journal)) | \(.sources_digested_with_the_journal), \(.sources_digested_without_the_journal) of \(.sources) | \(.nothing_ran) | \(.the_rewrite_in_place_is_missed_by_the_journal) | \(.the_rewrite_in_place_is_found_by_the_hermetic_pass) |"' "$file"
    done
    echo
    echo "A revision is atomic (the importing process ended from outside at random moments, a reader after each):"
    echo
    echo "| Cell | Assets | Another process | An uninterrupted pass ms | Passes | Ended before they finished | Readers with the former revision | Readers with the new revision | Unreadable or mixed | Cache entries at the end | Invalid entries | Unfinished files left | First problem |"
    echo "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .a_revision_is_atomic | "| \($cell) | \(.assets) | \(.the_importing_process_is_another_process) | \(.an_uninterrupted_pass_ms) | \(.passes) | \(.passes_ended_before_they_finished) | \(.readers_that_obtained_the_former_revision) | \(.readers_that_obtained_the_new_revision) | \(.unreadable_or_mixed_revisions) | \(.cache_entries_at_the_end) | \(.invalid_cache_entries) | \(.unfinished_files_left) | \(.first_problem) |"' "$file"
    done
    echo
    echo "Packages (built from the scenes and their closure; a later package from what two manifests say is new, mounted with and without leave to override):"
    echo
    echo "| Cell | Entries | Domain package bytes | Domain package hash | Domain slice identical from two caches | Presentation slice identical from one cache | From two caches | Later package: entries, already mounted, new | Later package hash | Overrides the entries it names and no other | Refused for each entry already mounted |"
    echo "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |"
    for file in "${facts[@]}"; do
      jq -r '.cell as $cell | .packages | "| \($cell) | \(.entries) | \(.domain_package_bytes) | \(.domain_package_hash) | \(.domain_slice_identical_from_two_caches) | \(.presentation_slice_identical_from_one_cache) | \(.presentation_slice_identical_from_two_caches) | \(.later_package_entries), \(.later_package_entries_already_mounted), \(.later_package_entries_new) | \(.later_package_hash) | \(.overrides_the_entries_it_names_and_no_other) | \(.refused_for_each_entry_already_mounted) |"' "$file"
    done
    echo
    distinct="$(jq -r '.profile + " " + .packages.domain_package_hash + " " + .packages.later_package_hash' "${facts[@]}" | sort -u | wc -l | tr -d ' ')"
    echo "- Distinct pairs of package hashes (the domain package and the later package) across every cell, per profile: $distinct."
    echo
    echo "A scene-sized text source (its import into canonical bytes with its validation):"
    echo
    for file in "${facts[@]}"; do
      jq -r '.scene_sized_source as $s | "- `\(.cell)`: \($s.prototypes) prototypes, \($s.source_bytes) source bytes, \($s.canonical_bytes) canonical bytes; \($s.best_ms) ms at best of \($s.rounds), \($s.best_nanoseconds_per_prototype) ns per prototype at best, \($s.median_nanoseconds_per_prototype) at the median; the part is the pipeline'"'"'s \($s.the_part_is_the_pipeline_s)."' "$file"
    done
    ;;
  *)
    echo "No summary is defined for this spike; the raw facts are in the artifacts."
    ;;
esac
