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

For Claude Code through `clauding`, use the Anthropic origin without `/v1`; the SDK appends `/v1/messages` itself. For the same-host LM Studio profile with a 153,600-token window:

```json
{
  "baseUrl": "http://127.0.0.1:18180",
  "model": "qwen36-35b",
  "contextTokens": 153600
}
```

Use each backend's actual context limit. Start a new `clauding` process after changing its config. A custom-model recognition warning does not establish request failure; validate an actual reply and session continuation. [Executed conversation check](../tests/reports/interoperability/20261112_clauding.md).

## LAN deployment with same-host LM Studio

The source repository also includes ready-to-run Windows x64 and macOS arm64/x64 binaries in [deploy/v0.1.0](https://github.com/hikaMaeng/apibond/tree/main/deploy/v0.1.0), with a version manifest and SHA-256 verification. They require neither Node nor Cargo to run. Each binary release has its own `deploy/v<version>/` directory; local runtime configuration stays separate from these versioned artifacts. These repository artifacts are separate from the npm wrapper's download/install path.

[examples/lmstudio-lan.json](../examples/lmstudio-lan.json) exposes `qwen36-35b` over all local IPv4 interfaces on port 18180, backed by LM Studio instances `qwen36-35b:28` and `qwen36-35b:tuf` on the same host:

```sh
apibond --config examples/lmstudio-lan.json --check
apibond --config examples/lmstudio-lan.json
```

`listen: "0.0.0.0:18180"` accepts both loopback and the host's assigned IPv4 addresses without hardcoding a LAN address. Clients use `http://<current-host-address>:18180/v1`; `0.0.0.0` is the bind address. The same-host LM Studio URL is `http://127.0.0.1:12345/v1`, so changing the host's LAN address does not break the upstream connection. The standard `--init` example remains loopback-only. IPv6 is not enabled by this IPv4 listener.

The LAN example retains explicit inbound auth `none`; configure keys as above when needed. A wildcard listener does not configure Windows Firewall. Same-host requests to a LAN address verify binding, not another machine's firewall traversal. This example admits four requests per backend (eight total); it is distinct from the 16-per-backend concurrency-test configuration. Modify a separate deployment config to change capacity or model names. The root `apibond.json` is intentionally untracked.

Optional backend health_path is origin-relative, e.g. `/health`; until its first successful probe the backend is unavailable. Optional queue_path returns `{"queue":N}` with bounded Content-Length. Capacity defaults 64; select a value appropriate to the backend. Full pinned backend returns 503, preserving locality.

`transport.header_timeout_ms` and `body_idle_timeout_ms` default null. Connect timeout defaults 10s. Do not set short inference deadlines for long prefill/non-streaming reasoning. Ctrl+C/SIGTERM drains requests for shutdown_timeout_secs (default 30s).

`runtime.worker_threads` controls native Tokio workers. null defaults to min(logical cores, 8), avoiding unnecessary workers on large hosts; choose 1..512 explicitly for your load. Requests >=1 MiB are inspected on the bounded-admission blocking pool so large multimodal JSON does not monopolize streaming workers.

Source build: `cargo build --release --locked`, then run target/release/apibond[.exe]. An npm Git URL is also supported: `npm install -g github:hikaMaeng/apibond#v0.1.0`. Release tgz embeds binary hashes; Git installs retrieve hashes from the same hosted release. Hash checks detect corruption, not an independently signed release identity. If scripts are disabled, first CLI use performs the same install; a read-only global prefix falls back to the user's cache.
