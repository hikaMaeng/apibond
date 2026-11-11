APIBond는 동일 모델의 여러 API 서버를 세션별로 묶는 Rust 스트리밍 프록시입니다.

| Goal | File |
|---|---|
| 목적 | [Overview](docs/overview.md) |
| API | [API reference](docs/api.md) |
| 설치·설정 | [Usage](docs/usage.md) |
| 제약 | [Constraints](docs/constraints.md) |
| 검증 | [Testing](docs/testing.md) |
| 구조·내부 | [Architecture](docs/architecture.md), [Internals](docs/internals.md) |
| 사전 검토 계획 | [Development plan](docs/development-plan.md) |

```sh
npm install -g https://github.com/hikaMaeng/apibond/releases/download/v0.1.0/apibond-0.1.0.tgz
apibond --init
# Edit apibond.json: configure your endpoints, model names and credentials.
apibond --check
apibond
```

OpenAI Chat Completions, Anthropic Messages, OpenAI Responses 및 네이티브 Files HTTP API를 지원합니다. 요청은 해당 API를 지원하는 backend로 전달합니다. 세션 헤더는 `x-apibond-session-id` 또는 `x-session-id`입니다.

설치기·런처만 Node를 사용하며 추론 트래픽은 Rust가 처리합니다. npmjs 레지스트리 게시는 별개이며, 위 명령은 GitHub Release 패키지를 설치합니다.
