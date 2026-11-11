# Testing

```sh
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
node --test scripts/install.test.mjs
cargo build --release --locked
cargo run --release --example bench -- target/release/apibond
```

On Windows the benchmark executable argument is target/release/apibond.exe. Benchmark creates work/bench-config.json and work/benchmark.json, launches only its own native proxy child, and terminates it on completion. It uses real loopback HTTP streams, not paid inference. Results are fixture transport evidence rather than model tokens/sec.

Tests cover native API/query/header/error pass-through, 1 MiB multimodal payload preservation, exact model splice, decoded duplicate keys, concurrent first-session affinity, sequential session distribution, pinned capacity, shared-backend CAS, streamed cancellation, unknown/conflicting state, response lifecycle, native file references/upload/content, late ID after 8 MiB output, compressed observed responses, no implicit header deadline, credential replacement/isolation, chunked body limits and memory admission.

Installer tests cover platform names, pinned artifact paths, embedded hash verification, corrupted downloads, tampered cache rejection and lazy install behavior. Release verification must additionally exercise actual npm --ignore-scripts installation from the public tgz and CLI startup.

Performance matrix: 1 KiB/1 MiB/16 MiB opaque JSON data, unchanged/rewrite/raw paths, concurrency 1/64, alternating direct/proxy batches after warmup. Records first body byte p50/p95/p99, total time, inter-chunk gaps, batch rate/errors and externally sampled native process memory. Dedicated 50-chunk SSE producer includes 2ms pacing. Input size and sample count are reported per result; no fixed low-overhead claim without measurement.

The benchmark runs client, mock upstream and proxy in separate processes. SSE events carry source timestamps; producer pacing, receive gaps and emission-to-receive latency are reported separately. This distinguishes upstream/OS timer jitter from relay delay. Add --stress-only after the binary argument for the two-worker/four-60-MiB-image stress case; output is work/benchmark-stress.json. Normal matrix uses eight proxy workers; effective config and binary hash are embedded in every report.

Execution plan: [integration plan](../tests/plans/acceptance-20261111.md). Actual results belong in tests/reports/acceptance/ with timestamps. UI/browser tests are N/A (no UI).

Real LM Studio evidence is separate from fixture transport measurements:

* [Prefix matrix plan](../tests/plans/prefix-matrix-20261111.md) and [12-request prefix result](../tests/reports/prefix/20261111_214520.md): same-session repetition, suffix variation, new-session reuse on an explicitly selected backend, cross-backend cold input, and early-prefix mutation. Includes compact per-request JSON; visible answers and cache usage are separate criteria.
* [Real concurrency result](../tests/reports/concurrency/20261111_195247.md): 8/16/32 sessions, upstream LM Link errors, client deadline limits, control-plane responsiveness and permit recovery. A balanced proxy admission count is not a successful two-engine inference result or a measurement of upstream running slots.
