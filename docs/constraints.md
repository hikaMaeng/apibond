# Constraints and state contract

## State selection

Sessions are scoped to inbound credential and public model pool. Native handle lookup is scoped to credential/provider/ID; its owner must belong to the selected model pool. When inbound auth is explicitly none, callers share one scope. IDs colliding across backends are treated as unknown and require a pin.

1. Known state owner wins; conflicting session/explicit backend is 409.
2. Otherwise use an explicit backend belonging to the model/API pool.
3. Otherwise use an existing compatible session pin.
4. Otherwise return 409. A newly presented session ID is not an existing pin.

| State | Behavior |
|---|---|
| Responses IDs / previous_response_id | Track from root JSON ID or streamed response.id |
| Uploaded file IDs | Track upload JSON; inspect native input_file/input_image/container_upload/file/source references |
| Tool result / function output content | Recurse structured native content; tool argument strings remain opaque |
| conversation, prompt.id, vector store IDs, containers, skills, item_reference | Require existing pin or explicit backend; not independently tracked |
| metadata/schema examples named file_id | Ignored; not media references |

No native-state migration or inference POST replay. Connection failures cool down a backend; stateless next requests may repin. A known state owner that is unavailable returns 503. An existing session changing to an unsupported API family returns 409; start a separate session or select a compatible owner.

Maps are bounded and in-memory. Idle TTL/LRU eviction and router restart can remove ownership. Recover with the original x-apibond-backend and model header or existing compatible session. Multi-router deployments require ingress stickiness; no shared state store is included. KV residency/eviction inside inference servers is not observed.

Session mappings default to one hour idle TTL and 10,000 entries per model pool. Accepted requests and stream completion/drop refresh last-use time. Maintenance removes expired inactive sessions at `transport.health_interval_ms` (default 10 seconds, plus probe/scan time); selection also checks the requested session's expiry. At capacity, LRU may evict an inactive entry before TTL; active sessions are preserved. Native handle ownership has a separate 24-hour idle TTL and 100,000-entry global LRU. Removing a mapping does not delete upstream KV, and upstream KV eviction does not automatically remove a mapping. Maintenance currently scans each session map under its mutex; maximum-size cleanup latency is not established by small-session tests.

Backend capacity is admission, not a waiting queue: when no eligible backend has a free slot, selection returns 503. `queued=0` with no queue_path configured is not proof that an upstream engine has no queued work. With no model-specific health probe, `available=true` does not prove that the configured model remains loaded or that LM Link is healthy. An upstream may answer a requested model name using another loaded model; endpoint pinning alone cannot guarantee physical cache locality in that case.

## Raw path

Disabled by default; model pool allow_raw=true requires identical upstream model names. Use x-apibond-model plus x-apibond-raw=true and JSON body containing that upstream model. Chat/Messages creation only. Body bytes stream without validation or rewrite; sessions must come from headers.

Raw is a trusted optimization: body model, file handles and native state are unchecked. It is not a model-access-control boundary; backend credentials may authorize other models. Restrict backend credentials appropriately. Responses, count_tokens and Files do not accept raw mode.

## Memory and transport

Inspected JSON default 64 MiB/request and 256 MiB aggregate reservation. Reserve twice Content-Length (unknown length: twice per-request maximum) before reading, reject admission with 503. After reading, shrink reservation to twice actual length. The remaining permit is attached to shared Bytes ownership and released when the last upstream upload reference is freed, independently of inference response time. This budgets body buffers and allocator growth, not total process RSS; session/owner metadata, transport buffers and observer stack add bounded per-request overhead. Upload default 512 MiB, enforced while streaming; partial bytes may already reach upstream when an oversized streamed body is aborted.

Resource lifecycle URL IDs accept ASCII letters, digits, underscore and hyphen; path-normalization characters are rejected. Native tool input arguments remain opaque, including objects with arbitrary keys/types. tools:null and native optional file_ids:null pass through.

Observed responses request identity encoding; unexpected compressed success is 502 before forwarding success headers. Other compressed response bytes pass through unchanged. No media fetching, redirects, automatic inference retries or unbounded intermediate stream channel. Downstream stalls hold request/backend permits, bounded by configured global/backend limits. A client that neither reads nor disconnects may hold its slot until transport close or shutdown.

HTTP inference/native Files routes only: no semantic protocol translation, provider admin/batch routes, stored Chat lifecycle, Conversations CRUD, Realtime or Responses WebSockets. TLS ingress is supplied externally; upstream HTTPS uses rustls/platform trust roots. System proxy environment variables are deliberately ignored.
