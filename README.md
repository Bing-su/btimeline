# btimeline

FFLogs 전투 로그를 수집하고 타임라인을 작성하기 위한 도구입니다. 타임라인 생성 기능은 새 계약으로 다시 설계 중입니다.

새 계약과 구현 순서는 [타임라인 재설계 계획](PLAN.md)을 참고하세요.

## 현재 사용 가능한 명령

| 명령                            | 용도                                       |
| ------------------------------- | ------------------------------------------ |
| `btimeline fflogs`              | 보고서의 전투 목록 확인 및 로그 다운로드   |
| `btimeline fflogs-collect`      | 보고서·전투 후보를 찾아 다운로드 계획 저장 |
| `btimeline fflogs-download`     | 저장된 계획에 따라 다운로드 또는 재개      |
| `btimeline inspect LOG.json...` | 수집된 로그 검사                           |

각 명령에 `--help`를 붙여 사용법을 확인하세요. FFLogs 인증 정보는 환경 변수 `FFLOGS_CLIENT_ID`, `FFLOGS_CLIENT_SECRET` 또는 작업 디렉터리의 `.env`에서 읽습니다. 기존 환경 변수가 우선합니다.

## 개발

```sh
cargo test
cargo clippy --all-targets -- -D warnings
cargo +nightly fmt --all -- --check
```

FFLogs 모의 서버 테스트에는 로컬 HTTP 포트가 필요합니다.
