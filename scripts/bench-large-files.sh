#!/bin/sh
# Large-file benchmark: opens generated files (1 MB to 100 MB, a 5 MB single
# line, a million short lines) in a headless Strata window and times opening,
# frames, typing, scrolling, select all + copy, find, save and the hot-exit
# backup. Each case runs in its own process so the memory column (RSS growth
# after opening) isn't skewed by the cases before it. Files are generated
# once into $TMPDIR/strata-bench.
#
#   scripts/bench-large-files.sh              # every case
#   scripts/bench-large-files.sh 50mb-rs      # some cases (see CASES in src/app/bench.rs)
#
# STRATA_BENCH_SPIN=<phase> repeats one phase for 8 s, to attach a profiler.
set -e
cd "$(dirname "$0")/.."
cases=${*:-"1mb-rs 10mb-json 50mb-rs 50mb-rs-full 100mb-log 100mb-log-full 5mb-line 5mb-line-full 1m-lines backup"}
cargo test --release --no-run --quiet
for c in $cases; do
    STRATA_BENCH_CASES=$c cargo test --release --quiet large_file_bench -- --ignored --nocapture --test-threads=1 2>/dev/null |
        grep -Ev '^(running|test )|^[[:space:].]*$'
done
