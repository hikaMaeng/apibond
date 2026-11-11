# HTTP API

| Method | Path | Backend capability |
|---|---|---|
| POST | /v1/chat/completions | openai |
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

| Header | Contract |
|---|---|
| x-apibond-session-id | Stable conversation identity; highest priority |
| x-session-id, x-claude-code-session-id, x-codex-session-id | Session fallbacks in that order |
| x-apibond-model | Public alias; required for file uploads/list and unknown URL resources |
| x-apibond-backend | Explicit backend ID, constrained to selected model/API pool |
| x-apibond-api | Files family: openai or anthropic; native Anthropic version/beta headers also identify Anthropic |
| x-apibond-raw | true: trusted opt-in raw Chat/Messages path |

Inspected JSON accepts `metadata.session_id` after header fallbacks; optional `user`/`metadata.user_id` fallback is disabled by default. A request ID never selects a session. The response includes `x-apibond-backend`, useful for recovery of native handles after router restart.

Inspected bodies require application/json or application/*+json (parameters accepted), identity content encoding, and a public `model`. Known Responses continuation can infer alias from its previous response owner. Root model is rewritten to the configured upstream model; other bytes are unchanged.

Files uploads require a multipart/form-data boundary and x-apibond-model. Every Files call must identify its family; conflicting header indications are rejected. Listing files selects one backend; it does not merge provider pagination across servers.

Inbound credentials: one Bearer Authorization or x-api-key. All inbound credentials/routing headers are stripped before configured backend credentials are injected. Local errors use Anthropic envelopes on Messages/native Anthropic paths, OpenAI envelopes otherwise. Unknown path/method 404, malformed input 400, conflicting state 409, oversize 413, wrong content encoding/type 415, capacity 503, transport failure 502, configured header timeout 504.
