# APIBond development plan

Status: established revision 3. Claude Opus/high rereview authorized implementation after four bounded corrections; all four are incorporated below. No further planning gate is required by that review.

## User acceptance criteria (verbatim)

러스트 기반의 본딩서버를 f:\dev\apibond 에 러스트로 작성하라.

1. 설정파일은 별도의 json으로 분리
2. 관련 레포도 깃헙에 공개로 작성
3. 간단히 npm으로 인스톨 가능해야 함
4. 서버는 앤트로픽 api와 openai api를 지원해야하고 더 나아가 responses api도 지원해야함
5. 그외에는 위의 라우터 구현의 유의점을 최대한 준수하여 최고 성능이 나도록 구현
6. 설정에서는 본딩할 정보를 구성함
7. 하나의 모델이름으로 어떤 서버를 후반에 두고 그 서버가 어떤 api에 대응하는지 기술

사용자 요청은 멀티모달을 포함한 복잡한 양상이 될수 있으므로 사전에 충분한 개발 계획과 클로드리뷰를 거쳐 확립한 뒤 진행

Prior routing/transport cautions referenced by criterion 5 (verbatim):

- 요청 처리: ID를 헤더에 두고, 큰 프롬프트 JSON의 전체 파싱·재직렬화를 피함
- 응답 처리: SSE를 토큰별로 해석·재구성하지 않고 받은 바이트를 바로 전달
- 연결 관리: upstream 연결 풀과 keep-alive로 연결·TLS 협상 재사용
- 부하 조회: 매 요청마다 모든 서버에 물어보지 않고 카운터·주기적 보고 사용
- 흐름 제어: 느린 클라이언트에 맞춰 읽기를 조절하고, 취소 시 upstream도 중단
- 스트리밍이라도 중간에서 JSON을 해석하거나, 청크를 모으거나, 무제한 큐에 쌓으면 지연과 메모리가 늘어납니다.
- 또한 중계기를 거치는 네트워크 경로 자체의 비용은 남습니다.
- 추가 첫 응답 지연, 청크 전달 지연의 p95/p99, 동시 연결 증가에 따른 CPU·메모리를 측정해야 합니다. 긴 프롬프트 업로드도 포함해야 요청 본문 버퍼링 비용이 드러납니다.

Mapped below to JSON/raw path, inline response stream, pooled transport, local load accounting, cancellation guards, and the performance evidence matrix respectively. This server does one structural validation pass in inspected mode; it avoids provider SDK object conversion, nested payload decoding, and whole-document reserialization. Raw mode removes this structural pass under its explicit trust contract.

## Product contract

- A Rust library plus server executable; Node is only the installer/launcher, never the data plane.
- A public model alias selects a configured pool of same-model backends. Global backend definitions declare base URL, supported API families, optional credentials from environment variables, capacity and cooldown. Model pools reference backend IDs plus their upstream model names; a physical backend shared across aliases has one global capacity counter.
- Native OpenAI Chat Completions, Anthropic Messages, and OpenAI Responses requests are routed to backends declaring that family. Native request and response formats are retained; protocol translation would require a separate adapter, because it changes tool, reasoning, multimodal and state semantics.
- Session affinity means `(client credential scope, public model, session ID) -> backend`; request IDs are not session IDs.
- A new/unidentified session selects the least occupied eligible backend; existing sessions remain pinned while available. No token-by-token backend switching.
- One router process in this version. Multi-process affinity requires a shared store or ingress stickiness and is documented rather than silently approximated.

## Configuration

- Separate strict JSON config with schema version, listen address, required inbound_auth ({mode: keys, key_envs: [...]} or explicit {mode: none}), transport, routing, body limits, model pools, backend capabilities and credentials. Missing auth mode, empty key lists in keys mode and missing env credentials fail startup/--check. Explicit none logs a startup warning.
- Native Rust standalone repository, not a TypeScript/Express monorepo; npm package is only a thin native-binary distribution wrapper.
- Example contains no real credentials. Local `apibond.json` and work files are ignored.
- Backend `base_url` is an API prefix (e.g. `http://host:8000/v1`); route suffixes append without double `/v1`. Reject invalid URLs, fragments, userinfo, duplicate IDs, empty pools, impossible limits, unsupported capabilities and missing configured env credentials at startup.
- Accept one inbound `Authorization: Bearer KEY` or `x-api-key: KEY`; reject ambiguous multiple credentials. Compare SHA-256 key digests in constant time. Always strip `authorization`, `x-api-key`, `api-key`, `proxy-authorization`, `x-apibond-*`, routing session headers and hop-by-hop headers before upstream forwarding. Backend `auth: {scheme: auto|bearer|x-api-key|none, env: ...}` controls injection; auto uses x-api-key for Anthropic and bearer for OpenAI. No implicit BYOK; with auth none caller credentials are still stripped. Custom headers are validated at startup, cannot redefine auth/routing/framing headers, and override other caller headers. Preserve native version/beta headers unless configured otherwise. Never log bodies/credentials.
- Health checks are optional per backend with configurable path; active in-flight counts always work without backend telemetry. Polling is outside the request path. Initial availability and health statuses must be explicit.
- `--config`, `--check`, `--init`, `--version`, and `--help` CLI modes; log startup and graceful shutdown.

## HTTP surface and multimodal scope

- `POST /v1/chat/completions`, `POST /v1/messages`, `POST /v1/messages/count_tokens`, `POST /v1/responses`, `POST /v1/responses/compact`.
- `GET/DELETE /v1/responses/{id}`, `GET /v1/responses/{id}/input_items`, `POST /v1/responses/{id}/cancel` use ownership established at creation. Query strings preserved.
- Native `/v1/files` upload/retrieve/content/delete and list with explicit routing context, to keep uploaded file references on their owner. Upload multipart bodies are streamed unchanged and require `x-apibond-model`; native Anthropic beta headers pass through. File ID ownership is scoped by API family and caller.
- `/v1/models` returns public aliases/capabilities in OpenAI form by default and Anthropic list form when anthropic-version is supplied; model/status routes use configured inbound authentication. `/healthz` is minimal public liveness. `/status` contains only backend IDs/load/readiness and bounded routing counts.
- Unknown or unsupported paths/methods return explicit errors; no accidental open forward-proxy behavior.
- JSON body preservation: custom serde visitor decodes root keys into Cow/strings and keeps values as borrowed RawValue/spans. Reject decoded duplicate routing keys (including escaped spellings) rather than map-overwrite. Extract model/routing metadata; splice only the root model JSON string using shared Bytes prefix/new model/suffix segments when it differs. Preserve non-model bytes. Never deserialize/re-serialize whole documents into provider SDK types.
- Preserve arbitrary nested input content, base64, remote image URLs, audio, documents, video references, encrypted reasoning, tools, tool results, cache controls, unknown fields and numeric formatting. Never fetch user-supplied media URLs inside the router.
- Bounded extraction in native input/messages content subtrees (including nested tool_result/function_call_output content): discriminator input_file.file_id, input_image.file_id, container_upload.file_id, Chat content[].file.file_id, Anthropic source.type=file/source.file_id, and code-interpreter file_ids. Recurse at most depth 128 through structured native content, excluding arbitrary metadata/schema/tool argument strings. Reject conflicting tracked owners. Unknown file IDs use the unified state-owner rules below; strict_file_ownership=false allows unknown files upstream-native handling instead.
- Untracked native handles (Responses conversation, prompt.id, vector_store_ids, container IDs, item_reference.id and Anthropic container/skills) use the same pin requirements; they are not auto-migrated. Document tracked/untracked state table. Same-account sharing is not inferred.
- `x-apibond-backend` is accepted only if it belongs to the selected alias and supports the request family. For `/v1/files`, `x-apibond-api: openai|anthropic` selects native file family; native anthropic-version/beta headers can identify Anthropic, conflicting indications are 400. Otherwise require the explicit API header, including on retrieval/list; do not guess from inbound credential scheme.
- Configure max buffered JSON bytes (64 MiB), aggregate JSON budget (256 MiB), max open requests and upload bytes. Reserve budget before reading: known Content-Length reserves 2x length for allocator growth, unknown length reserves 2x per-request maximum. Reservation never waits while holding a partial body. Byte limits apply to chunked bodies as well. Oversize is 413, admission exhaustion is 503. Byte slices for model rewrite share the original allocation.
- Header-directed fast path: pool `allow_raw` defaults false and requires all bindings to have the same upstream model (published as raw_upstream_model by /v1/models). With `x-apibond-model` and `x-apibond-raw: true`, Chat/Messages creation bodies stream unchanged; caller uses that common upstream model. Raw trust contract: body model and file/state references are unchecked; backend credentials can access any model they permit. Enable only for trusted callers with appropriately restricted backend credentials. No claim of model enforcement or file ownership validation in raw mode. Reject raw without model header, raw on Responses/count_tokens/files, raw on a non-opt-in pool, or raw on a pool with differing model names. Explicit header model/backend conflicts still rejected.
- Multipart is opaque; select by model/session/explicit backend, stream with byte limit and bounded flow control.
- Standalone binary supports HTTP upstream and HTTPS via rustls, HTTP/1.1 and HTTP/2 pools. TLS ingress can be supplied by an external reverse proxy. Responses WebSocket mode, Realtime, batch processing, provider admin APIs and cross-provider file translation are outside the HTTP inference contract and explicitly documented.

## Affinity, ownership and concurrent admission

- Session precedence: x-apibond-session-id, x-session-id, x-claude-code-session-id, then native metadata.session_id. user/metadata.user_id fallback is off by default and enabled only by explicit config. A request ID is never used.
- Bound ID length/maps/idle TTL. Refresh on use; never evict active sessions. At capacity evict least-recently-used idle entry; validate max_session_entries > max_open_requests. Opaque ownership maps may evict their oldest entry, with explicit unknown-state recovery semantics.
- Global backend capacity uses atomic compare-and-swap upper-bound reservation; per-model locks protect first-session insertion only and never span awaits. Choose by active/capacity, then live-session/capacity, then rotating tie break. Pinned full backend defaults to reject (503); no silent remap. Cross-family request with existing session pin on an incapable backend returns 409; caller explicitly starts a new session or selects a compatible owner. This preserves identity and avoids guessing whether native context is portable.
- Separate per-backend capacity guards from global request and buffered-body guards. Hold capacity through entire streamed response consumption, release on body drop/error/cancellation/EOF. No background unbounded channel that detaches upstream reads from downstream demand.
- Unified state-owner decision (all under the same caller scope): known tracked owner wins, with 409 on any conflicting session/explicit backend; otherwise use a validated explicit backend; otherwise an existing same-alias, API-capable session pin; otherwise 409. A new session ID is not an existing pin. Apply uniformly to files, previous_response_id, lifecycle URLs and untracked native handles; never infer owner across scopes. Response-map eviction with a surviving session pin is recoverable through that pin.
- Only connect-stage failure (DNS/TLS/refusal/connect timeout) starts cooldown; invalidate session binding only if it is still owned by that backend and no other request is active. Header/application/idle timeout does not imply unhealthy backend or globally invalidate mappings. Do not automatically replay inference POSTs; server processing/tool side effects may have started.
- A stateless request (no native state handles) may be reassigned on its next request if its pinned backend is unavailable. Native state handles are non-migratable; apply unified owner selection and require its owner to be available or return 409/503. Raw mode observes header session IDs only and cannot prove state absence.
- Track successful response/file IDs with a constant-memory incremental JSON lexical observer. It only retains container states, bounded key/ID strings and escape state (max depth 128, keys 256 bytes, IDs 512 bytes), skipping large string values. Match root id for JSON/files or response.id for SSE response.created. Continue through arbitrarily late id fields without storing full output/events. Publish owner before yielding the chunk that identifies it, then stop observing. SSE supports comments, CRLF, split frames/UTF-8 and multiline data fields. Malformed/unrecognized metadata never alters forwarded bytes; later unknown handles fail closed unless explicitly pinned.
- Preserve backpressure inline, with no detached task/channel. For observed requests force upstream Accept-Encoding: identity; reject unexpected non-identity encoding before sending response headers because ownership cannot be safely inferred. Other routes preserve encoded bytes unchanged. No chunk-level reserialization or parsing of generated content after ID discovery.
- Response/file maps have bounded sizes and idle TTL; no false cache-residency guarantee. Process restart loses state and unknown IDs return conflict. Client may supply explicit backend routing for recovery; never silently fail over native state.
- Scope identity by a SHA-256 fingerprint of the inbound credential (never return/log it); if inbound authentication is disabled all callers share one documented scope. Backend secret values loaded once at startup.

## Low-overhead transport

- Axum/Tokio/Reqwest with shared clients/pools and TCP_NODELAY. Connect timeout defaults 10s. Header and body-idle timeouts default disabled for inference so long prefill and non-streaming reasoning remain valid; optional explicit per-request transport limits are documented. Health probes have independent short deadlines. Shutdown drain has a finite deadline. No implicit whole-response timeout.
- Automatic content decompression disabled; keep original bytes/Content-Encoding. Reject compressed inspected JSON requests; raw mode forwards encoded bodies only under explicit contract.
- Strip fixed hop-by-hop headers and those named by `Connection`, `Host` and stale framing headers; preserve SSE, usage, rate-limit, native error status/body and tracing headers. Disable upstream redirects to avoid secret forwarding and wrong POST semantics.
- Response bytes forwarded as `Bytes`, no per-token SDK objects or JSON serialization. A small ownership observer is enabled only for handle-producing requests.
- Backend load uses local active request/capacity ratio, not synchronous fanout. It estimates occupancy, not actual backend token queue; report it accurately.
- No Redis, database, media fetching, synchronous disk logging, protocol conversion or per-chunk metrics on the hot path. Model rewrite uses shared prefix/suffix Bytes segments plus a tiny new JSON string; measure this path separately from raw mode.
- Bound user-space buffers; slow client cancellation drops upstream stream. Graceful shutdown drains active work within configured deadline.

## Source layout and dependencies

- `src/config.rs`: schema/defaults and startup validation.
- `src/inspect.rs`: borrowed JSON metadata/model replacement, bounded response-ID probe.
- `src/routing.rs`: pools, atomic selection/reservation, affinity/opaque ownership and RAII guards.
- `src/proxy.rs`: HTTP surface, authentication, request admission, headers, pooled transport and inline stream forwarding.
- `src/lib.rs`, `src/main.rs`: public router constructor and CLI.
- `tests/`: real loopback mock upstream integration tests; no paid API calls needed.
- `scripts/`: native npm install, launcher and repeatable performance benchmark.
- Dependencies version-pinned in Cargo.lock; minimal features and no provider SDK. Inspect current stable crate APIs before choosing exact versions.
- README index and concise `docs/{overview,architecture,api,usage,constraints,internals,testing}.md` per package-docs-writer skill.

## Acceptance tests

1. Config validation covers capabilities, URL/path joining, missing env secrets, duplicates, zero limits and header injection.
2. All three inference APIs, Anthropic token-count endpoint, responses compaction/lifecycle, query preservation, native status/header/error pass-through.
3. Multimodal fixtures with large base64 images/audio, URLs, PDF/file references, mixed nested content, escaped strings, non-ASCII, tool calls/results, encrypted/unknown fields and exact preservation of bytes outside root model span.
4. Root duplicate model/routing fields, malformed/overdeep JSON, compressed bodies, absent model, wrong content type, chunked/Content-Length body overflow, global buffered-memory pressure.
5. Parallel first requests with same session hit one owner; unidentified requests distribute based on load; sessions across API capabilities, credential isolation, TTL/capacity bound and pinned-backend saturation.
6. Response-created event split at every boundary, SSE comments/CRLF, non-streaming IDs, subsequent `previous_response_id`, lifecycle methods, unknown/missing owner, conflicting file/session/state owner, backend outage with non-migratable state.
7. Multipart upload and streamed file download, uploaded ID tracking, file reference routing and explicit-owner recovery.
8. Streaming first chunk arrives before upstream completes; exact byte preservation and errors after headers; cancellation and slow consumers release load/admission permits; peak buffered memory remains bounded under concurrency.
9. Credentials replaced upstream, inbound auth rejected where configured, hop-by-hop/Connection-nominated headers removed, no redirect credential leakage; status does not expose secrets.
10. `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --locked`, native release build, npm installer tests, clean package inspection and real isolated npm install/launch/config-check.
11. Review regressions: all raw invalid combinations cause 4xx and zero upstream calls; allowed raw body bytes unchanged; observed upstream Accept-Encoding identity and unexpected gzip rejected before success headers; root ID after 8 MiB output and late SSE metadata registered in bounded observer memory; connect=1s/headers delayed 3s succeeds without inference deadline; incompatible-family pin gives 409; files-family conflict/missing family gives 400; uploaded input_image/container_upload routing and item_reference pin requirement; metadata/schema/tool-argument file_id ignored; response map eviction with surviving session succeeds, without pin gives 409; omitted auth/empty keys/missing env fail startup, explicit none succeeds with warning.

## Performance evidence

- Compare direct vs proxy against the same local mock producer, interleaving samples after connection warmup. Report fixture results separately from real model inference (no model throughput claim).
- Include 1 KiB, 1 MiB and 16 MiB JSON payloads, matching-model and model-rewrite paths, raw request streaming, SSE 50+ chunks, concurrency 1 and 64.
- Record time to response headers/first body byte, inter-chunk delivery gaps, total wall time, p50/p95/p99, errors and completed requests/sec; report hardware/runtime and sample sizes. Chunk arrival is not assumed to equal token arrival.
- Low-load small-payload design target: added p50 TTFT under 1 ms, p99 under 5 ms on loopback; this is a target to measure, not a claimed result. Report deviations and optimize actual hot spots.
- Include a slow-client/cancellation fixture; a low-latency result hiding unbounded buffering does not pass. Record OS process working set/RSS externally as well as internal byte permits. Rust loopback benchmark is the primary latency measurement; Node benchmark/fixture helpers, if used, are labeled end-to-end harness measurements and not transport CPU capacity claims.

## Packaging and publication

- Public GitHub `hikaMaeng/apibond`, MIT license, source, Cargo.lock, sample config, CI and release workflow. Secret scan before staged named paths are committed/pushed; preserve unrelated work.
- npm package apibond is a zero-dependency Node launcher/install script. Download version-pinned raw native binary assets over HTTPS, check SHA-256 before atomic placement, fail on corruption/unsupported OS. Release tgz is generated after native assets and embeds their hashes. Git-source installation uses release SHA-256 files (TLS/hosted-release trust, corruption detection rather than independent authenticity). Launcher lazily runs the same installer if scripts were disabled; test npm --ignore-scripts. Node waits for the Rust child but does not handle inference traffic.
- GitHub Release matrix: Windows x64, Linux x64/arm64, macOS x64/arm64. CI tests on hosted OSs before native artifact build/upload. Verify actual artifact availability before claiming support.
- Use current hosted runner labels ubuntu-24.04, ubuntu-24.04-arm, windows-latest, macos-15 and macos-15-intel. Linux glibc baseline is Ubuntu 24.04 unless musl targets are built and tested; state the baseline honestly.
- The machine has GitHub authentication but no npm registry login. Fulfill simple npm install using a public release `.tgz` URL and/or `github:hikaMaeng/apibond#v0.1.0`; registry `npm install -g apibond` is only claimed if publication is actually authenticated and verified.
- Exercise a fresh install in an isolated project-owned prefix/cache and start the installed binary against mock upstreams. Verify public visibility, commit SHA, release assets/checksums and CI run outcomes.

## Review gate and implementation sequence

1. Independent Claude Opus / high read-only review of this plan; ask for concrete correctness/performance counterexamples and executable acceptance criteria.
2. Validate findings directly, revise plan, and obtain a focused rereview for unresolved P1/P2 design problems. No runtime implementation before the plan gate is resolved.
3. Implement cohesive Rust data plane/config/ownership, integration tests and native npm distribution.
4. Run acceptance checks, inspect implementation independently with Claude, fix verified defects, measure direct/proxy overhead.
5. Commit scoped sources, create/push public repository, run release matrix, verify fresh npm installation and deliver concise links/configuration/validation limits.

## Current evidence

- F:/dev/apibond did not exist; created new empty Git main repository. No pre-existing user files were changed.
- Windows native Rust 1.96.0, Node 26.4.0, npm 11.17.0 and Claude Code 2.1.306 available. GitHub CLI account hikaMaeng is authenticated; requested repository name is available.
- Official OpenAI Responses and Anthropic Messages API pages fetched before plan drafting. No actual inference credentials or multimodal provider calls have been used.
- Registry name apibond is currently unregistered (npm view returned 404); npm whoami returned ENEEDAUTH. Public GitHub Release tgz installation is an explicit acceptance path, without claiming npmjs registry publication.

## Initial review dispositions

- P1-1/2/3/4 accepted: inference timeout defaults disabled; observer encoding identity; raw opt-in/common-model trust contract; explicit inbound/upstream credential rules.
- P2-1/2/3/4/5 accepted: decoded duplicate keys; native-only file paths; untracked state table/pin requirement; upfront body reservation; global CAS capacity.
- P2-6/7 accepted: session count + rotating load ties; idle LRU at map cap, never active session eviction.
- P2-8: choose explicit 409 for incompatible family instead of automatic repin, which the reviewer also identified as the required deterministic decision; prevents accidental context migration.
- P2-9/10/11/12 accepted: explicit Files family; constant-memory late-ID scanner; embedded release hashes + lazy install; original routing cautions included above.
- P3 accepted: shared Bytes rewrite; native models list shape; user fallback off; scope-safe unknown-state errors; process RSS evidence; precise gitignore. Optional connect retry, SQLite/Redis, static-musl builds are deferred.
- Rereview P2-1/2/3/4 accepted and incorporated: expanded native content handles, unified known/explicit/session/409 state table, explicit inbound_auth mode, and test 11. Rereview explicitly states these corrections close the planning gate without another review round.
- HTTP inference surface also includes POST /v1/responses/input_tokens if supported upstream; stored Chat completion lifecycle, Conversations CRUD, realtime and websocket remain explicitly outside this release. Their request-side native handles require a session/explicit backend pin.
- Inspected content type permits application/json with parameters and application/*+json; missing/other content type is 415. File upload requires multipart/form-data with boundary. Local error envelope matches Anthropic for Messages routes and OpenAI otherwise.
