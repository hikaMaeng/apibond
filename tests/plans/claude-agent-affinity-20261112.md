# Test Plan: Claude agent affinity

Created: 2026-11-12, Asia/Seoul.

Goal: retain KV locality for each agent history while distributing independent Claude Code agents sharing a root session across both model instances.

Environment: Windows, installed clauding/Claude Code 2.1.306, LM Studio on loopback port 12345, qwen36-35b:28 and qwen36-35b:tuf (10 parallel slots each). Isolated trial directory; do not modify the user's active project or terminate its clients.

Preconditions: preserve current source/config bytes; record running /status; build patched binary on a separate loopback listener before production cutover.

Steps:
1. Reproduce old behavior: shared X-Claude-Code-Session-Id and distinct x-claude-code-agent-id values collapse to one backend.
2. Verify HTTP integration: independent agents distribute, each agent remains pinned across follow-ups, root namespaces stay separate, explicit APIBond session takes precedence, state owners/capacity/cancellation retain existing contracts.
3. Run the user's YouTube search/transcript/original-and-summary webapp task through installed clauding, asking for multiple parallel agents and a youtubescript subdirectory. Capture sanitized request identity, selected backend, HTTP result, streaming usage and simultaneous /status snapshots, plus LM Studio server logs.
4. Repeat the real task after any correction until both instances receive independent agent requests and repeated agent identities retain the same backend. Distinguish model/tool/dependency failures from routing errors.
5. Drain and replace the production binary only after passing checks; keep versioned artifacts and scoped source changes reviewable.

Expected: both model instances active during overlapping independent agent requests; no agent owner changes; no false success from merely replaying synthetic IDs. Report task completion separately from routing acceptance.

Logs: local private request ledger/CLI transcript and LM Studio logs under ignored work/agent-routing-20261112; commit only sanitized aggregate evidence. Browser locator contract: N/A for router; any generated webapp acceptance is reported separately.
