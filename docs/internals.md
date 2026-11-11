# Internals

- Public Rust entry points: Config::load/validate, App::new, router(Arc<App>), App::spawn_maintenance. Embedders own their listener, maintenance task and graceful shutdown.
- Inspected JSON uses a custom object visitor and borrowed RawValue fields. Decoded duplicate keys are rejected. Unchanged requests use one Bytes body; model aliases use shared prefix/new JSON string/suffix segments.
- Completed JSON reads release excess reservation. Bytes::from_owner ties remaining permits to actual upload buffer lifetime; even a one-segment body uses a one-shot stream. Large inspection tasks retain body and admission permits while on spawn_blocking, including if the caller disconnects.
- Incoming JSON is coalesced into bounded 64 KiB pieces; contiguous assembly and inspection of >=1 MiB bodies run off the streaming workers. Buffered/raw upstream uploads are sliced to <=64 KiB and yield execution every four chunks to preserve fairness under immediate loopback writes.
- Native content extraction is restricted to input/messages structured subtrees and known built-in tool state. Unknown extension fields and large base64 strings are not decoded into provider SDK objects.
- Session keys use SHA-256 inbound-key scope plus stable session ID inside a model pool. Backend capacity uses one atomic counter across aliases. Equal occupancy considers live sessions, then a rotating tie breaker.
- Selection sorts immutable snapshots of counters. Bounded LRU maps avoid full-map eviction scans on state insertion; active sessions cannot be evicted. Expiry sweeps run in maintenance.
- Selection RAII guard is captured by the response body stream. EOF, error, dropped response, failed send and cancellation release capacity. Response body forwarding has no detached producer task.
- Incremental ID observer stores JSON structural frames, bounded keys/ID strings and escape state. It skips scalar payload strings, handles IDs after large output and observes SSE data lines without rebuilding events. Depth limit 128, retained string token bound 4096 bytes, decoded key 256 and ID 512.
- Header filtering removes hop-by-hop headers including Connection-nominated fields, stale Content-Length, Host and inbound credentials. Original content headers/errors survive. Upstream content length is recalculated for rewritten request bodies.
- Maintenance and status return backend IDs/load, not URLs or credentials. Logging omits request/response bodies and secrets.
