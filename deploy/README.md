# 버전별 배포

배포 바이너리를 Git 저장소에 직접 포함합니다. Git LFS 포인터가 아닌 실제 실행 파일입니다.

```text
deploy/
  v0.1.0/
    win32-x64/apibond.exe
    apibond.example.json
    manifest.json
    SHA256SUMS
```

| 버전 | 플랫폼 | 실행 파일 |
|---|---|---|
| 0.1.0 | Windows x64 | [apibond.exe](v0.1.0/win32-x64/apibond.exe) |

`v0.1.0`은 공개 GitHub Release의 Windows x64 바이너리와 동일한 바이트를 사용합니다. MSVC CRT 정적 링크 빌드이며 Node·Cargo 설치 없이 실행합니다. 원본 릴리스·런타임 소스 커밋·설정 예제 출처·바이너리 크기와 SHA-256은 [manifest.json](v0.1.0/manifest.json)에 기록합니다.

저장소 루트에서 검증하고 실행합니다. 두 번째 명령은 기존 운영 설정 `apibond.json`을 사용합니다. 신규 환경에서는 동봉 예제를 별도 운영 설정으로 복사한 뒤 모델 이름을 맞춥니다.

```powershell
.\deploy\v0.1.0\win32-x64\apibond.exe --config .\deploy\v0.1.0\apibond.example.json --check
.\deploy\v0.1.0\win32-x64\apibond.exe --config .\apibond.json
```

운영 설정은 버전 폴더의 배포 원본과 분리합니다. 루트 및 배포 폴더 아래의 `apibond.json`은 Git에서 제외됩니다. 동봉 예제는 모든 로컬 IPv4 주소에서 포트 18180을 수신하고, 같은 PC의 LM Studio에는 루프백으로 연결하므로 고정 LAN IP가 필요하지 않습니다. 자세한 설정은 [usage](../docs/usage.md)를 따릅니다.

새 배포는 `deploy/v<새 버전>/`을 추가합니다. 기존 버전의 바이너리·설정 예제·manifest·체크섬은 교체하지 않습니다. 동일 버전의 다른 플랫폼은 별도 플랫폼 하위 폴더와 해당 manifest 항목으로 추가할 수 있습니다.

소스 체크아웃에서 `npm run check:deploy`는 각 버전의 파일 크기·SHA-256·manifest·설정 JSON을 검증합니다. CI도 Windows·Linux·macOS에서 이 검사를 실행합니다. `deploy/`의 텍스트는 LF, 바이너리는 변환 없이 저장해 체크아웃 시 체크섬이 유지됩니다. `SHA256SUMS`는 바이너리뿐 아니라 manifest와 설정 예제도 포함합니다.
