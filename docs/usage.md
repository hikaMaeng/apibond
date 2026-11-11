# Installation and configuration

Node >=20 is needed by the npm wrapper. Prebuilt targets: Windows x64, Linux x64/arm64 (glibc >=2.39), macOS x64/arm64 (runner deployment target). Verify the release's actual assets for a version.

```sh
npm install -g https://github.com/hikaMaeng/apibond/releases/download/v0.1.0/apibond-0.1.0.tgz
apibond --init
apibond --config apibond.json --check
apibond --config apibond.json
```

`--init` never overwrites an existing file. Edit endpoints before starting; the sample localhost:8001/8002 servers are placeholders. Inbound auth is explicitly `none` in the local sample and generates a startup warning. For remote service use explicit keys and environment-backed backend credentials:

```json
{
  "schema_version": 1,
  "listen": "127.0.0.1:18180",
  "inbound_auth": { "mode": "keys", "key_envs": ["APIBOND_KEY"] },
  "backends": {
    "gpu-a": {
      "base_url": "http://192.168.0.10:8000/v1",
      "apis": ["openai", "responses"],
      "capacity": 32,
      "auth": { "scheme": "bearer", "env": "GPU_A_KEY" }
    },
    "gpu-b": {
      "base_url": "http://192.168.0.11:8000/v1",
      "apis": ["openai", "anthropic", "responses"],
      "capacity": 64,
      "auth": { "scheme": "auto", "env": "GPU_B_KEY" }
    }
  },
  "models": {
    "my-model": {
      "backends": [
        { "backend": "gpu-a", "model": "upstream-model-a" },
        { "backend": "gpu-b", "model": "upstream-model-b" }
      ]
    }
  }
}
```

`base_url` includes the API prefix (usually `/v1`); the proxy appends suffixes such as `/messages`. Schemes: none strips all caller credentials; bearer and x_api_key select a fixed upstream header; auto chooses x-api-key for Anthropic and Bearer for OpenAI. Missing configured env values fail startup. `headers` supplies other fixed upstream headers. Native anthropic-version and anthropic-beta are forwarded unless configured otherwise.

OpenAI clients use base URL `http://127.0.0.1:18180/v1`, Anthropic clients `http://127.0.0.1:18180`. Set model to the public alias and retain a stable session header across turns.

Optional backend health_path is origin-relative, e.g. `/health`; until its first successful probe the backend is unavailable. Optional queue_path returns `{"queue":N}` with bounded Content-Length. Capacity defaults 64; select a value appropriate to the backend. Full pinned backend returns 503, preserving locality.

`transport.header_timeout_ms` and `body_idle_timeout_ms` default null. Connect timeout defaults 10s. Do not set short inference deadlines for long prefill/non-streaming reasoning. Ctrl+C/SIGTERM drains requests for shutdown_timeout_secs (default 30s).

`runtime.worker_threads` controls native Tokio workers. null defaults to min(logical cores, 8), avoiding unnecessary workers on large hosts; choose 1..512 explicitly for your load. Requests >=1 MiB are inspected on the bounded-admission blocking pool so large multimodal JSON does not monopolize streaming workers.

Source build: `cargo build --release --locked`, then run target/release/apibond[.exe]. An npm Git URL is also supported: `npm install -g github:hikaMaeng/apibond#v0.1.0`. Release tgz embeds binary hashes; Git installs retrieve hashes from the same hosted release. Hash checks detect corruption, not an independently signed release identity. If scripts are disabled, first CLI use performs the same install; a read-only global prefix falls back to the user's cache.
