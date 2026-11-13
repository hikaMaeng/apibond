# Overview

- 공개 모델 alias 하나에 여러 backend와 각 backend의 실제 모델명을 연결한다.
- backend마다 `apis: ["openai", "anthropic", "responses", "embeddings"]` 중 지원 항목을 지정한다. `embeddings`는 unreleased source에서 추가된 별도 capability다.
- 기존 세션은 같은 backend로, 신규 요청은 `(active + queue) / capacity`가 낮은 backend로 보낸다. 동률이면 세션 수와 순환 순서로 분배한다.
- API 형식은 네이티브 그대로 유지한다. 서로 다른 API 사이의 의미 변환은 수행하지 않는다.
- 이미지·오디오·문서·도구 호출·암호화된 reasoning과 확장 필드를 보존한다. 미디어 URL을 프록시가 내려받지 않는다.
- Responses와 파일 ID의 소유 backend를 추적한다. 상태가 다른 서버로 잘못 전달되는 경우 409로 거절한다.
- API 프록시로서의 세션 affinity이며 실제 KV 캐시 잔존 여부를 보장하지 않는다.
