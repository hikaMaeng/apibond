# Test Plan: Review fixes

Created: 2026-11-13. Tracking: [issue #1](https://github.com/hikaMaeng/apibond/issues/1).

## Goal

Verify four approved fixes against base `2895464`: stalled inspected JSON admission, unsuccessful session reassignment, stale queue telemetry, and case-insensitive Bearer scheme parsing. Preserve native payloads, affinity, fail-fast capacity, and long inference behavior.

## Environment and preconditions

- Windows native Rust/Node, loopback HTTP fixtures on ephemeral ports.
- Separate implementation worktrees; final verification checks out the integrated candidate and builds it independently.
- Production port/configuration and versioned deployment artifacts remain untouched. No real-model calls are required for these proxy-state defects.
- Record compiler/Node versions, source commit/tree, commands, exits and test counts. Browser/UI testing: N/A.

## Cases and expected results

| Case | Trigger | Expected result |
|---|---|---|
| JSON idle admission | Two incomplete unknown-length JSON uploads exhaust a small configured budget | Configured JSON input idle deadline returns 408; reservations and global request slots recover; subsequent valid request succeeds |
| Progress versus total duration | JSON arrives in chunks with gaps below the idle deadline, total transfer exceeds it | Request completes without an overall upload deadline |
| Inference independence | Complete request followed by delayed headers or SSE longer than the input deadline | Native response completes; input deadline does not truncate inference |
| Configuration | Omitted timeout, explicit null, zero | Default is 60000ms; null disables this deadline; zero fails validation |
| Failed reassignment | Idle session pinned to unavailable A; B has no free admission slot | Request fails; original mapping/counters remain; recovered A retains session |
| Successful reassignment | A unavailable; B can accept | Session moves once to B; following requests and per-backend session counts agree |
| Queue fallback | Real maintenance polls valid high queue, then HTTP failure/invalid response, then valid queue | Failed sample is excluded via zero fallback; new requests use local active load; later valid telemetry is restored |
| Authentication | Bearer/bearer/BEARER with identical valid and invalid keys | Valid spellings succeed; token case/content remain exact; duplicate credentials remain rejected |

## Execution and evidence

1. Workers show new targeted cases fail on unchanged base and pass after the corresponding fix. Distinguish harness failures from product failures.
2. Integrate only owned files; check full Rust suite, Node installer tests, `cargo fmt --check`, Clippy with warnings denied, and deployment manifest/checksum validation.
3. Independent verifier builds the integrated commit in a separate checkout, reviews the diff, runs the tests and representative actual-binary HTTP cases, and posts a per-criterion verdict to issue #1.
4. Push only after independent PASS; verify remote HEAD and CI status. Report any remaining CI result as pending rather than passed.

Retain worker and independent execution logs outside committed source; commit a compact report under `tests/reports/review-fixes/`. Test outcomes and model inference performance are separate claims.
