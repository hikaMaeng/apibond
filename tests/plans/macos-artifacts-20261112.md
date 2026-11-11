# Test Plan: local macOS artifacts

Created: 2026-11-12, Asia/Seoul. Goal: add locally compiled Apple Silicon and Intel executables to deploy/v0.1.0 without changing the existing Windows artifact or profile.

Environment: two user-owned local-network Apple Silicon Macs; macOS 27.0.1, existing Rust/Cargo 1.97.1 and SDK 27.0. The builder already has aarch64-apple-darwin and x86_64-apple-darwin targets. Rosetta is absent. Read canonical local SSH information at each connection, keep host-key checking enabled, and use isolated temporary directories. Do not alter existing services, security configuration, keychains or global toolchain defaults.

Steps:

1. Archive the exact v0.1.0 runtime source commit 85452fb999621c755087cc8e4297d8a8e06b48e7, verify archive digest after transfer, and use Cargo.lock with two build jobs.
2. Run ARM tests and build ARM and Intel release targets with process-scoped MACOSX_DEPLOYMENT_TARGET=11.0. If the test environment's descriptor limit interferes, preserve that result and record any process-scoped limit adjustment separately.
3. Inspect Mach-O architectures, deployment target, dynamic dependencies and SHA-256. Transfer ARM to the second local Mac and execute native HTTP fixture checks. Do not install Rosetta just to test Intel.
4. Copy new artifacts into darwin-arm64/apibond and darwin-x64/apibond; append manifest entries and extend SHA256SUMS. Preserve existing Windows/config bytes, mark Mac files executable in Git, and verify raw Git blobs.
5. Run checksum checks on Windows/Linux/macOS CI and execute the committed Mac binaries on their native Apple Silicon and Intel CI runners using scripts/smoke-deployment.mjs.

Expected: both binaries report version 0.1.0 and pass native config/HTTP checks on their verified targets. HTTP fixtures cover Chat SSE, Anthropic Messages, Responses ownership, session affinity and slot/buffer release. These fixtures do not claim real-model performance. macOS 11.0 is a build target, not exhaustive old-OS validation.

Logs: build/test outputs including initial failures and runner limits, toolchain/SDK/architecture, artifact sizes/digests, second-Mac native result, Git mode/blob hashes and CI run. No SSH credentials or machine-specific connection details are committed.

Locator contract: N/A; CLI/HTTP test.
