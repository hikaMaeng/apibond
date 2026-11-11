# Review dispositions

Reviewed with Claude Code 2.1.306, actual model claude-opus-5-5, effort high, tools Read/Grep/Glob.

| Review | Result | Resolution |
|---|---|---|
| Initial plan | REVISE | Timeout/encoding/raw trust/auth rules and state/admission contracts revised |
| Focused plan rereview | Four bounded corrections; no further planning gate needed after correction | Native handles expanded; owner precedence unified; explicit inbound auth; regression matrix added |
| Implementation | Three P2, six P3; release allowed after fixes and routine checks | All three P2 and six P3 addressed below |

Implementation P2:

1. Tool argument traversal/null optional fields: content/output-only traversal, opaque tool input, nullable tools/file_ids; byte-preservation regression test.
2. Chunked body budget: shrink after read; attach remaining permit to Bytes::from_owner; use one-shot upstream body streams. Three delayed-header chunked requests prove upload memory is released before inference completes.
3. Multimodal inspection blocking runtime: >=1 MiB inspection moves to spawn_blocking while retaining byte permits. Benchmark includes four nested 60 MiB images concurrent with SSE on two native workers.

Implementation P3:

- Missing model: inspected creation rejects missing model, except native Responses continuation with previous_response_id; known ownership may infer alias and upstream can inherit the stored model.
- Path normalization: lifecycle resource IDs restricted to ASCII alphanumeric/underscore/hyphen; raw TCP regression checks encoded traversal without client-side normalization.
- Sorting: capture active/queue/session counters before comparing; capacity still reserved by CAS.
- Map cap: bounded LRU maps provide constant-time state eviction; active sessions are protected.
- Windows signals: npm wrapper does not force-kill on console Ctrl+C, allowing native drain.
- Benchmark: full header/first-byte/chunk distributions, concurrency 1/64, nested multimodal mixed load, slow readers/cancellation, effective config/runtime and binary SHA-256 metadata.

Additional performance decision: native worker count defaults to min(logical cores, 8) and can be set through JSON. Preliminary loopback measurements on the 48-logical-core Windows host showed lower CPU use and better large-body latency with fewer workers. Final measurements record their explicit worker setting; this is not a universal optimal-thread-count claim.

Full original prompts/results are retained in the requesting chat's local outputs, outside the public repository. No paid provider inference was used for acceptance fixtures.
