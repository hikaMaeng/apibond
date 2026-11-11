# Test Plan: APIBond acceptance

Created: 2026-11-11 (Asia/Seoul).

Goal: Verify native protocol/multimodal fidelity, session/state routing, bounded streaming/admission, installation and measured transport overhead.

Environment: Native Windows Rust/Node, loopback HTTP mock upstreams; GitHub-hosted Windows/Linux/macOS release jobs. No production model keys required.

Preconditions: cargo/node available, reviewed plan established; no user-owned upstreams or processes modified. npm release tests use an isolated work/ prefix/cache.

Steps and expected results:

1. cargo fmt/clippy/test --locked: all tests pass; no warnings. Inspect integration cases in tests/proxy.rs and observer boundary tests.
2. node installer tests: corrupt binaries rejected; verified fixtures installed/lazily reused.
3. Build release; run Rust loopback benchmark against native child. Record samples/errors/latency/RSS; do not label fixture results real model inference.
4. Independent Claude code review against established plan; reproduce P1/P2, fix confirmed findings, rerun affected checks.
5. Stage only owned source/docs/lock/config example/workflows. Secret scan and package file inventory exclude work/, local config and credentials.
6. Verify public repository/commit, release matrix status and binary/hash/tgz assets.
7. npm install public tgz with --ignore-scripts into isolated prefix; first CLI use downloads/verifies binary. --version/--init/--check and native API mock smoke pass.

Logs: test summaries, benchmark JSON, review report, CI jobs/commit/release IDs, npm install and CLI results. No prompts/bodies/credentials from real providers.

Locator contract: N/A; no UI.
