# HTTP API

| Method | Path | Backend capability |
|---|---|---|
| POST | /v1/chat/completions | openai |
| POST | /v1/embeddings | embeddings (unreleased source) |
| POST | /v1/messages, /v1/messages/count_tokens | anthropic |
| POST | /v1/responses, /v1/responses/compact, /v1/responses/input_tokens | responses |
| GET, DELETE | /v1/responses/{id} | responses |
| GET | /v1/responses/{id}/input_items | responses |
| POST | /v1/responses/{id}/cancel | responses |
| POST, GET | /v1/files | openai or anthropic |
| GET, DELETE | /v1/files/{id} | openai or anthropic |
| GET | /v1/files/{id}/content | openai or anthropic |
| GET | /v1/models, /status | local; configured inbound authentication |
| GET | /healthz | public liveness |

Capability means the upstream implements that route. Declaring a family does not add missing routes to a backend. Query strings and native error responses pass through. Responses WebSocket mode is outside this HTTP contract.

**Unreleased source:** `embeddings` is a separate capability from `openai` Chat Completions. Embedding-only backends do not receive Chat/Messages/Responses requests; Chat-only backends do not receive embeddings requests. Published v0.1.1 binaries do not include this route or configuration value.

Embeddings accepts native `input` as a string, string array, token-ID array, or array of token-ID arrays. The router only reads routing fields and rewrites the root model alias; it forwards the complete batch in one upstream request without splitting, reordering, re-encoding, or applying Responses/file ownership rules. `encoding_format`, `dimensions`, `user`, and other fields remain unchanged. The backend validates inputs and options. Response vectors, `data[].index`, model, usage, encoding, and native errors pass through unchanged. Raw mode is not available for embeddings.

| Header | Contract |
|---|---|
| x-apibond-session-id | Stable conversation identity; highest priority |
| x-session-id, x-claude-code-session-id, x-codex-session-id | Session fallbacks in that order |
| x-claude-code-agent-id | With the Claude session fallback, gives each agent its own stable affinity within that root session |
| x-apibond-model | Public alias; required for file uploads/list and unknown URL resources |
| x-apibond-backend | Explicit backend ID, constrained to selected model/API pool |
| x-apibond-api | Files family: openai or anthropic; native Anthropic version/beta headers also identify Anthropic |
| x-apibond-raw | true: trusted opt-in raw Chat/Messages path |

Inspected JSON accepts `metadata.session_id` after header fallbacks; optional `user`/`metadata.user_id` fallback is disabled by default. A request ID never selects a session. The response includes `x-apibond-backend`, useful for recovery of native handles after router restart.

Since 0.1.1, a Claude session plus agent ID identifies one independent agent history. Shared parent-agent IDs do not merge child histories. Missing agent IDs retain root-session affinity; higher-priority `x-apibond-session-id` or `x-session-id` deliberately overrides agent separation. A bare agent ID without a Claude session does not establish affinity. Older clients that send only a shared root ID cannot distribute their branches through this header mechanism. `RUST_LOG=apibond=debug` logs the selected backend and a hashed routing identity, without prompt or credential contents.

Inspected bodies require application/json or application/*+json (parameters accepted), identity content encoding, and a public `model`. Known Responses continuation can infer alias from its previous response owner. Root model is rewritten to the configured upstream model; other bytes are unchanged.

Files uploads require a multipart/form-data boundary and x-apibond-model. Every Files call must identify its family; conflicting header indications are rejected. Listing files selects one backend; it does not merge provider pagination across servers.

Inbound credentials: one Bearer Authorization or x-api-key. In unreleased source the authentication scheme is case-insensitive; the key remains exact. All inbound credentials/routing headers are stripped before configured backend credentials are injected. Local errors use Anthropic envelopes on Messages/native Anthropic paths, OpenAI envelopes otherwise. Unknown path/method 404, malformed input 400, conflicting state 409, oversize 413, wrong content encoding/type 415, capacity 503, transport failure 502, configured header timeout 504. Unreleased source additionally returns 408 when the configured inspected JSON input idle deadline expires; published v0.1.1 binaries do not include this deadline or the authentication scheme change.
