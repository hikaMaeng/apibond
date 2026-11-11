# 버전별 배포

배포 바이너리를 Git 저장소에 직접 포함합니다. Git LFS 포인터가 아닌 실제 실행 파일입니다.

```text
deploy/
  v0.1.0/
    win32-x64/apibond.exe
    darwin-arm64/apibond
    darwin-x64/apibond
    apibond.example.json
    manifest.json
    SHA256SUMS
```

| 버전 | 플랫폼 | 실행 파일 |
|---|---|---|
| 0.1.0 | Windows x64 | [apibond.exe](v0.1.0/win32-x64/apibond.exe) |
| 0.1.0 | macOS Apple Silicon | [apibond](v0.1.0/darwin-arm64/apibond) |
| 0.1.0 | macOS Intel | [apibond](v0.1.0/darwin-x64/apibond) |

Windows 바이너리는 공개 GitHub Release의 v0.1.0 파일과 동일하며 MSVC CRT 정적 링크 빌드입니다. Mac 바이너리는 같은 런타임 소스 커밋을 로컬 Mac에서 Rust 1.97.1로 빌드했습니다. Apple Silicon용 네이티브 빌드와 Intel용 크로스 빌드를 제공하며 macOS deployment target은 11.0입니다. 의존 동적 라이브러리는 macOS 시스템 라이브러리뿐입니다. Developer ID 서명·공증 배포는 아닙니다. 원본·빌드 환경·소스 커밋·설정 예제 출처·바이너리 크기와 SHA-256은 [manifest.json](v0.1.0/manifest.json)에 기록합니다. 실행에는 Node·Cargo가 필요하지 않습니다.

저장소 루트에서 검증하고 실행합니다. 두 번째 명령은 기존 운영 설정 `apibond.json`을 사용합니다. 신규 환경에서는 동봉 예제를 별도 운영 설정으로 복사한 뒤 모델 이름을 맞춥니다.

```powershell
.\deploy\v0.1.0\win32-x64\apibond.exe --config .\deploy\v0.1.0\apibond.example.json --check
.\deploy\v0.1.0\win32-x64\apibond.exe --config .\apibond.json
```

Mac에서는 아키텍처에 맞는 경로를 사용합니다. Git 체크아웃에는 실행 권한이 포함됩니다.

```sh
./deploy/v0.1.0/darwin-arm64/apibond --config ./apibond.json
# Intel Mac:
./deploy/v0.1.0/darwin-x64/apibond --config ./apibond.json
```

macOS 로컬 검수에서 기본 열린 파일 한도는 256이었습니다. 대량 연결을 검수할 때는 해당 실행 셸에서 `ulimit -n 4096`으로 높였습니다. 시스템의 영구 설정이나 다른 서비스는 변경하지 않았습니다. macOS 11.0은 빌드 타깃 값이며, 모든 구형 OS 조합의 실기기 검증을 뜻하지 않습니다.

운영 설정은 버전 폴더의 배포 원본과 분리합니다. 루트 및 배포 폴더 아래의 `apibond.json`은 Git에서 제외됩니다. 동봉 예제는 모든 로컬 IPv4 주소에서 포트 18180을 수신하고, 같은 PC의 LM Studio에는 루프백으로 연결하므로 고정 LAN IP가 필요하지 않습니다. 자세한 설정은 [usage](../docs/usage.md)를 따릅니다.

새 배포는 `deploy/v<새 버전>/`을 추가합니다. 기존 플랫폼의 바이너리·설정 예제는 교체하지 않습니다. 동일 버전에 다른 플랫폼을 추가할 때는 별도 플랫폼 하위 폴더를 만들고 manifest와 체크섬 목록을 확장합니다.

소스 체크아웃에서 `npm run check:deploy`는 각 버전의 파일 크기·SHA-256·manifest·설정 JSON을 검증합니다. CI도 Windows·Linux·macOS에서 이 검사를 실행합니다. `deploy/`의 텍스트는 LF, 바이너리는 변환 없이 저장해 체크아웃 시 체크섬이 유지됩니다. `SHA256SUMS`는 바이너리뿐 아니라 manifest와 설정 예제도 포함합니다.

`node scripts/smoke-deployment.mjs <binary>`는 실제 배포 바이너리를 임시 루프백 서버로 실행하고 Chat SSE, Anthropic Messages, Responses 소유권, 세션 고정 및 슬롯 반환을 검사합니다. 실제 모델 추론·성능 측정과 구분되는 HTTP fixture 검수입니다. Mac CI는 Apple Silicon과 Intel 실행기에서 각각 해당 바이너리를 직접 실행합니다.
