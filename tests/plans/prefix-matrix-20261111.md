# Test Plan: prefix matrix

Created: 2026-11-11 (Asia/Seoul); procedure captured before the executed 21:45:20 run.

Goal: Separate stable routing, exact-prefix reuse, suffix changes, cross-session reuse on one backend, backend isolation, and early-prefix mutation.

Environment: APIBond v0.1.0 at loopback port 18180; same-host LM Studio port 12345; model alias qwen36-35b with qwen36-35b:28 and qwen36-35b:tuf. Deployment settings are not changed. Responses streaming, temperature 0.2, max_output_tokens 4096, store=false. Native HTTP client has no separate body-idle deadline; explicit overall deadline 300 seconds.

Preconditions: Both exact model IDs loaded; proxy idle. Use a run-specific marker near the beginning of a 5,299-character programming reference. Stable x-session-id per chain, new x-request-id per request. The first owner request and the new-session control explicitly select the backend; other requests omit that header. No previous_response_id is used.

Steps:

1. Seed :28 with the reference and `타입스크립트의 특징을 설명하라`.
2. After it finishes, start :tuf with the identical input. Continue one serial chain per backend, at most two requests in flight overall.
3. Repeat exactly, ask `러스트의 특징을 설명하라` with the same reference, then repeat the TypeScript input under a new session explicitly assigned to the same backend.
4. Change the reference's early A/B identity marker and ask `파이썬의 특징을 설명하라`, then repeat that mutated input exactly.

Expected: Repeat/suffix/new-session/mutated-repeat cache ratios exceed 90%; cold and early-mutated inputs are materially lower. Stable sessions stay on their selected backend and actual response.model matches. Record stream termination, visible explanations, and cached-token counts independently. Early-prefix mutation also changes the question to Python; it is not a whole-request single-character-only control.

Logs: Per-case input/body hashes, session/trace IDs, backend and response.model, usage.input_tokens_details.cached_tokens, first generated/visible times, total duration, visible body length, failures, final active/buffer counters. Preserve full local evidence under ignored work/; publish compact results without raw inference logs or generated reasoning.

Reproduction input: English reference covers type systems, execution, memory, concurrency, error handling, abstractions, ecosystems and interoperability. Input is a user reference, an assistant acknowledgement, then the exact Korean language question. Repetitions hold the entire body constant; only header IDs change for the new-session control. Use a fresh early marker for each new run; do not substitute warm input for cold measurements.

Locator contract: N/A, API-only test.
