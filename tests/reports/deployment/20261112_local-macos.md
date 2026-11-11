# Test Report: local macOS builds

Timestamp: 2026-11-12, Asia/Seoul. Local build/verification checkpoint before the artifact commit's hosted native-macOS CI.

Environment: user-owned local Apple Silicon builder and a second Apple Silicon verifier, both macOS 27.0.1. Existing rustc 1.97.1 (8bab26f4f 2026-07-14), cargo 1.97.1 (c980f4866 2026-06-30), SDK 27.0. No toolchain/target installation or global toolchain selection was needed. Build concurrency was limited to two jobs.

Source: exact runtime commit 85452fb999621c755087cc8e4297d8a8e06b48e7, locked dependencies. Transferred source archive SHA-256: 01bce40eba3a1e22b19d0aa9640d088f6df99d959fcec2c62a83d2b9053a81be. A first checksum guard had a manually mistyped expected digest and stopped before extraction/build; the corrected guard computes the expected digest from the actual local archive.

| Artifact | Target | Bytes | SHA-256 |
|---|---|---:|---|
| darwin-arm64/apibond | aarch64-apple-darwin | 6646208 | 327c8d72a6235d6c4336f422d87b5ae9031703fc4901046b3cffa6aecaf9d105 |
| darwin-x64/apibond | x86_64-apple-darwin | 7832340 | 56d093b3f6d1362bcb96335a8dca2c191ad45890ac9aab032144cce54cb5f0d2 |

Builds: cargo build --release --locked for each target, MACOSX_DEPLOYMENT_TARGET=11.0. Mach-O inspection reports arm64/x86_64 respectively, minos 11.0, sdk 27.0. otool lists only system Security/CoreFoundation/libiconv/libSystem dependencies. The ARM code signature passes strict verification; Intel is unsigned. No Developer ID signing/notarization or keychain changes were performed.

Tests and failure history:

- Initial default-parallel ARM tests passed 3 library and 19/20 integration tests; concurrent_first_session_is_atomic_and_new_sessions_balance observed one 502 instead of 200. This result is retained, not counted as an all-pass run.
- The failed test passed when run alone. The full 23 tests then passed serially.
- The SSH shell's soft descriptor limit was 256 and hard limit unlimited. With only that test process's shell soft limit raised to 4096, the full default-parallel suite passed 23/23. No permanent OS/service limit was changed. This is consistent with descriptor pressure in concurrent fixtures; the original syscall error was not captured, so precise causality is not asserted.
- On the second local Mac, the ARM release binary passed six native loopback fixture requests: Chat SSE, Anthropic Messages, Responses owner continuation, same-session affinity, new-session distribution, and permit/buffer recovery.
- That native-smoke harness initially reported a startup timeout because it did not remove ANSI styling from the already-successful listener log. The parser was corrected; the same binary then passed. Native child/temporary fixtures were cleaned by the harness.
- The same smoke script passed against the existing Windows release binary. npm run check:deploy verified all three platforms and all version checksums.

Intel execution: unavailable on the local ARM Macs because Rosetta is absent. The CI workflow adds a macos-15-intel job that runs the committed Intel executable natively, plus macos-15 for ARM. Their outcomes are post-push checks, not claimed by this local checkpoint. Existing Windows bytes and configuration example were not replaced.

Reproducibility: [plan](../../plans/macos-artifacts-20261112.md), version manifest, npm run check:deploy and node scripts/smoke-deployment.mjs <binary>. The local Mac builds intentionally have their own hashes and compiler provenance; they are not claimed byte-identical to GitHub Release's separately built Mac assets.

Limitations: no real model inference or general performance acceptance was run on these Mac binaries. macOS 11.0 is the link target; local execution was on 27.0.1, with older native hosted execution delegated to CI. Credentials, host addresses and remote workspace/account details remain outside Git.

Locator outcome: N/A; no UI.
