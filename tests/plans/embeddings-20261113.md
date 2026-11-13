# Test Plan: Native embeddings

Created: 2026-11-13. Tracking: [issue #2](https://github.com/hikaMaeng/apibond/issues/2).

## Goal

Add native `POST /v1/embeddings` with explicit `embeddings` backend capability. Preserve a batch as one request and rewrite only its model alias. Validate first against one live upstream, not a multi-node throughput test.

## Environment and preconditions

- Windows native Rust/Node; base `cc70887`, clean main before changes.
- Live upstream `http://192.168.0.6:12345/v1`, model `embeddinggemma2:mp`, existence confirmed through `/v1/models`.
- Candidate uses a separate loopback listener and single-backend JSON configuration. Existing chat configuration, process and affinity maps remain untouched.
- Official input contract: string, string array, token array, array of token arrays. See [OpenAI reference](https://developers.openai.com/api/reference/resources/embeddings/methods/create).

## Cases and expected results

1. Fixture: all four input forms, exact model splice and body preservation, one upstream call per batch, options such as encoding format/dimensions/user retained.
2. Fixture: native vector data, indexes, usage, float/base64 and error status/body unchanged. No Responses/file-state interpretation of embedding input.
3. Capability: embedding-only backend accepts embeddings and is ineligible for generative APIs. Chat-only backend is ineligible for embeddings. Model list declares the capability.
4. Actual binary + one live model: single string, one-element string batch and multiple-string batch all return expected counts and indexes with consistent dimensions. Compare proxied results with direct results and verify ordering.
5. Probe upstream token-input/base64 behavior separately. Unsupported upstream features must produce the same native failure through the proxy, not a false proxy success.
6. Full Rust suite, Node installer tests, format, Clippy and existing deployment integrity checks pass. Browser/UI and multi-node performance are N/A.

## Evidence

Record candidate/source identity, binary hash, runtime versions, API outcomes, vector counts/dimensions, numerical comparison and permit recovery. Full vectors and command logs stay in local execution evidence; commit a compact result report and configuration example. Published v0.1.1 binaries are distinct from the new source capability.
