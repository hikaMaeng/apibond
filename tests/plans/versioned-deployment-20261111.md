# Test Plan: versioned repository deployment

Created: 2026-11-11, Asia/Seoul.

Goal: Store the actual deployable Windows executable under a versioned repository directory, preserve byte identity across Git checkouts, and operate from that path.

Environment: Windows x64, Git, Node, existing APIBond deployment and same-host LM Studio. Existing runtime settings and upstream models are retained. No new model inference stress run is required for an artifact relocation.

Preconditions: clean starting tree; v0.1.0 published Windows release digest available; source binary's SHA-256 matches the release digest. Do not overwrite an already-staged version manifest. Preserve the existing live executable for rollback.

Steps:

1. Stage deploy/v0.1.0/win32-x64/apibond.exe, configuration example, source/version manifest and SHA256SUMS. Set binary Git attributes and LF for checksummed text.
2. Run npm run check:deploy; run the actual executable's --version and --check against the included example and current runtime settings.
3. Verify the staged/committed Git blob's byte count and SHA-256, confirming that Git holds the executable rather than an LFS pointer. Run installer tests and inspect the npm file inventory.
4. Push the owned paths; wait for Windows/Linux/macOS CI, including the new artifact verification step.
5. With the existing proxy idle, activate the repository-versioned executable. Verify its process path and wildcard listener, dynamic local addresses, actual backend/model streaming and same-session routing; cancel probes after the first generated delta and verify slot/buffer recovery.

Expected: executable size 7,638,016 bytes and SHA-256 40c9cb8832428088ccba748c6ce106116a58dbf99d5507201935661979554395; version 0.1.0; both configuration checks pass; all checksums/Git bytes stable; no fixed LAN address in the deployment profile. Runtime settings stay outside the immutable versioned files.

Logs: local checksum/native checks, Git blob digest, CI run and commit, active executable path, listener and smoke JSON. Preserve machine-local receipts under ignored work/ and user output artifacts; no credentials or private runtime config enter Git.

Locator contract: N/A, no UI.
