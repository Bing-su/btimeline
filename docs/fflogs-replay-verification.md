# FFLogs 입력 재생 확인

이 확인은 저장된 FFLogs 수집 파일이 `compare`를 통과하고 원본 이벤트에 연결되는지 살펴본 스모크 테스트다. 전체 전투 타임라인, ACT 신호, 실제 게임 또는 외부 실행기와의 일치 검증은 아니다. 입력 파일은 `logs/`에 있으며 Git에는 포함하지 않는다. 다른 환경에서 재현하려면 같은 수집 파일이 필요하다.

| 로컬 입력                                         | 이벤트 수 | 원본 배열의 시각 역전 | SHA-256                                                            |
| ------------------------------------------------- | --------: | --------------------: | ------------------------------------------------------------------ |
| `logs/old-susano-2026/g2YNCdhycKXkxMfJ_4.json`    |     7,358 |                     0 | `bf867223acdcf94baf43b36327c312780c106229c1fd26db9c6d7889c2d5d772` |
| `logs/expanded-2059/6XKb1zc9PdyMjRvC_7.json`      |    44,811 |                     1 | `f859f20584d008c8794a320288590a9329d622803db376478bc1695d30e70f1f` |
| `logs/autonomous-r10s/JKWX7gk1yVRBaNCZ_23.json`   |    23,744 |                     0 | `e1ece0ca83c61a27001a484162430d20e6edb90b460e568b7aac2fc3071210c2` |
| `logs/loop-meso-terminal/7xf2dqWzVNFrLCJn_3.json` |    18,169 |                     1 | `cfdbaff664f76d3805865195aaf976b4d716963a540f25623b397489c2f790da` |

시각 역전은 앞선 배열 원소보다 timestamp가 작은 원소의 개수다. 네 파일 모두 수집 검증과 재생 입력 변환을 통과했다. 뒤의 세 파일은 Susano 테스트 타임라인과 다른 전투이므로 입력 경로만 확인했다.

Susano 파일의 두 `begincast`만 확인하는 [스모크 테스트 KDL](examples/susano-fflogs-smoke.kdl)을 사용했다.

```sh
cargo run -- compare docs/examples/susano-fflogs-smoke.kdl logs/old-susano-2026/g2YNCdhycKXkxMfJ_4.json --json
```

| 결과                      | 전투 시작 기준 시각 | 원본 이벤트 인덱스 |
| ------------------------- | ------------------: | -----------------: |
| start와 phase 진입        |            17,101ms |                417 |
| `second-cast` 예정 이벤트 |            80,501ms |          해당 없음 |
| 두 번째 시전 sync         |            80,523ms |               2089 |

두 번째 예정 이벤트는 선택한 63.4초 시각에 표시되며, 원본 신호와 22ms 차이가 있다. 이 검사는 FFLogs 고유의 `FFLogs.begincast`만 사용한다. FFLogs의 이벤트와 액터 masterData는 [공식 API 문서](https://www.fflogs.com/v2-api-docs/ff/report.doc.html)에 설명돼 있으며, 이 검사는 ACT 신호와의 동등성을 입증하지 않는다.
