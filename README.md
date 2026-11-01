# btimeline

FFLogs 전투 로그를 수집하고 타임라인을 작성하기 위한 도구입니다. 타임라인 생성 기능은 새 계약으로 다시 설계 중입니다.

새 계약과 구현 순서는 [타임라인 스키마 초안](docs/TIMELINE_SCHEMA.md)과 [재설계 계획](docs/PLAN.md)을 참고하세요. cactbot 시작·리셋과 구간 전환의 근거는 [생명주기 조사](docs/CACTBOT_LIFECYCLE_RESEARCH.md)에 정리했습니다.

## 현재 사용 가능한 명령

| 명령                                               | 용도                                             |
| -------------------------------------------------- | ------------------------------------------------ |
| `btimeline fflogs`                                 | 보고서의 전투 목록 확인 및 로그 다운로드         |
| `btimeline fflogs-collect`                         | 보고서·전투 후보를 찾아 다운로드 계획 저장       |
| `btimeline fflogs-download`                        | 저장된 계획에 따라 다운로드 또는 재개            |
| `btimeline inspect LOG.json...`                    | 수집된 로그 검사                                 |
| `btimeline mine LOG.json... -o process.kdl`        | 관측된 시전의 직접 후속 관계를 KDL 그래프로 저장 |
| `btimeline validate timeline.kdl`                  | 타임라인의 구조·참조·명확한 범위 검사            |
| `btimeline compare timeline.kdl recording.json...` | 정규화한 관측의 오프라인 재생                    |

각 명령에 `--help`를 붙여 사용법을 확인하세요. FFLogs 인증 정보는 환경 변수 `FFLOGS_CLIENT_ID`, `FFLOGS_CLIENT_SECRET` 또는 작업 디렉터리의 `.env`에서 읽습니다. 기존 환경 변수가 우선합니다.

`mine`은 한 전투·난이도·언어의 수집 로그를 분석합니다. 같은 밀리초의 신호는 순서가 없는 묶음으로 취급하며, 그래프의 `follows`는 인접 관측 횟수입니다. `trace`는 각 풀의 시간, 원본 이벤트 인덱스, 해시와 클리어 여부를 보존합니다. 클리어한 풀의 마지막 관측만 별도로 집계하며 전투 종료 신호로 취급하지 않습니다.

이 그래프는 시전 시작·완료를 대상으로 하는 **분석 초안**입니다. 간선이 기믹 분기 조건이나 가능한 전체 경로라는 뜻은 아닙니다. 조기 전멸 이후는 관측되지 않으며, 서로 다른 시각에 기록된 동시 신호나 누락된 패킷 때문에 그래프가 달라질 수 있습니다. 페이즈·전투 시작과 리셋 신호의 추론, 다른 로그 타입, 최종 타임라인 자동 생성은 아직 구현되지 않았습니다.

`validate`는 구조 검증의 첫 구현이며, `compare`는 lifecycle·phase·event·sync만 재생합니다. 분기·반복이 있으면 재생을 거부합니다. 입력은 FFLogs 원본이 아닌 정규화 관측이며, 형식과 미검사 범위는 [오프라인 재생 안내](docs/OFFLINE_REPLAY.md)를 참고하세요.

## 개발

```sh
cargo test
cargo clippy --all-targets -- -D warnings
cargo +nightly fmt --all -- --check
```

FFLogs 모의 서버 테스트에는 로컬 HTTP 포트가 필요합니다.

| 테스트 도구 | 사용 범위                                                                                |
| ----------- | ---------------------------------------------------------------------------------------- |
| rstest      | 잘못된 KDL과 ECMAScript 정규식의 명명된 입력 사례                                        |
| proptest    | 원본 시각 평행 이동 시 결과 보존, 여러 시간 보정 후 이벤트 중복 실행 방지                |
| insta       | CLI 검증 요약과 전체 재생 JSON 스냅샷. 머신별 입력 경로만 제외하고 시각·실행 기록은 보존 |

스냅샷이 달라지면 생성된 `.snap.new`를 기대 동작 및 기존 `.snap`과 비교한 뒤 반영합니다. 출력 변경을 확인하지 않고 일괄 갱신하지 않습니다.

`scripts/`의 TypeScript 스크립트는 Bun으로 실행합니다. 스크립트 테스트는 `bun run --cwd scripts test`로 실행합니다. cactbot 비교·검증에는 별도로 의존성이 설치된 cactbot 체크아웃이 필요하며, 공식 검증은 해당 체크아웃의 `ts-node`와 Mocha를 사용합니다.
