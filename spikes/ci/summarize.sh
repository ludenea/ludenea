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
  *)
    echo "No summary is defined for this spike; the raw facts are in the artifacts."
    ;;
esac
