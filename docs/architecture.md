# Architecture

```text
Client -> authentication/admission -> API + model + session/state owner
       -> atomic backend capacity reservation -> pooled native upstream
       <- demand-driven Bytes stream <- optional constant-memory ID observer
```

| Module | Responsibility |
|---|---|
| config | Strict JSON configuration, URL/header/credential/limit validation |
| inspect | Borrowed raw JSON spans, native content handles, incremental ID observer |
| routing | Per-model session locks, shared backend CAS counters, scoped state maps |
| proxy | Native HTTP routes, body admission, header filtering and streaming |
| main | CLI, listener, background maintenance and shutdown drain |

Session selection takes a short per-model lock, without I/O while locked. Physical backend capacity is shared across model aliases and reserved by CAS. Responses/file ownership has a separate bounded map. Backend secrets and HTTP clients are loaded once.

Maintenance polls configured health/queue paths and expires idle maps outside the request path. `queue_path` returns JSON `{"queue": 3}` with Content-Length <= 16 KiB; `queue` means waiting requests excluding this proxy's active counter. Without telemetry, routing uses local active requests only.

Node downloads/verifies/launches a native child; it never receives inference HTTP data.
