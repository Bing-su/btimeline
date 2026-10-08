# P7 분기·페이즈 확장

구현일: 2026-10-06. SPEC v1의 label·event·sync jump만 사용한다. 독립 재생과 예고 목록 투영을 구현했으며 실제 cactbot parser/runtime 표시 검증은 실행하지 않았다.

## 사용

```sh
btimeline generate logs/example --lookahead 30 -o out/branch.yaml
btimeline validate out/branch.yaml
btimeline convert out/branch.yaml -o out/branch.txt
btimeline replay out/branch.yaml logs/example -o out/branch.replay.json
```

| 계약             | 동작·예시                                                                                                                                                                                                                                                                                                                                                |
| ---------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 입력             | P5의 동일 encounter/difficulty 그룹 선택·정규화·양방향 대응을 재사용한다. 전투별 ID·이름 조건문을 추가하지 않는다.                                                                                                                                                                                                                                       |
| 동시 신호 정렬   | 정렬용 복사본에서 보스 완료 anchor가 하나인 동시 묶음만 `SignalKey` 순서로 고정해 `보스+helper`와 `helper+보스`가 동일한 구간에 대응하도록 한다. helper만 있는 구간과 다중 보스 anchor 구간의 기존 문맥·원본 재생 순서·event index·instance 개수는 보존하며, 서로 다른 시각은 묶지 않는다. 다중 anchor의 순서 불확실성은 기존 양방향 합의 검사에 남긴다. |
| 분기 경계        | 공통 보스 완료 신호 사이 또는 fight start부터 첫 공통 보스 완료까지의 관측 구간을 사용한다. 합류를 보지 못한 wipe는 새 경로를 만들지 않는다.                                                                                                                                                                                                             |
| 공통 prefix      | `A→U→X,P→C`와 `A→U→Y,Q→C`에서 U는 한 번 출력하고 X/Y 이후를 별도 경로로 생성한다.                                                                                                                                                                                                                                                                        |
| 판별 신호        | 경로마다 첫 완료 cast의 능력 ID·source가 구분되어야 한다. 역할·instance 개수·start/cast 순서가 포함된 경로를 유지한다. ID 배열로 이미 표현 가능한 독립 대안은 P5 출력을 유지한다.                                                                                                                                                                        |
| 진입·합류        | X/Y 완료 행에서 경로 label로 sync jump한다. 각 경로에 C 완료 행을 두고 같은 merge label로 sync jump한다. 관측한 후속 행은 merge 진입 상대시간으로 배치한다.                                                                                                                                                                                              |
| 전환 window      | 입력별 이전 활성 sync/jump의 `(원본 시각, 가상 시각)`으로 다음 신호의 보정 시계를 계산한다. 중앙값을 0.1초로 반올림하고 관측 min/max를 포함하도록 before/after를 바깥 방향으로 올림한다. 기본 ±2.5초는 최소 여유로 유지한다.                                                                                                                             |
| 분기 선택 window | 모든 경로의 판별 신호가 도착한 보정 시계 범위를 각 selector의 window에 반영한다. X가 4초, Y가 10초에 관측되면 같은 fork에서 두 도착을 기다릴 수 있다.                                                                                                                                                                                                    |
| 페이즈           | 공통 완료 신호에 필요한 보정 window가 기본 ±2.5초보다 넓고 활성 sync로 표현 가능하면, 해당 행에 sync jump와 다음 페이즈 label을 추가한다. 관측 지연의 표시이며 HP 전환 원인을 추정하지 않는다.                                                                                                                                                           |
| 구간 배치        | 이전 구간의 최대 활성 종료 시각에 최장 관측 pull 길이·설정 lookahead·2.6초를 더해 다음 구간을 배치한다. 신호를 못 받은 채 시간이 흘러도 학습 입력의 길이 안에서는 다른 경로로 자연 진입하지 않는다.                                                                                                                                                      |
| lookahead        | `--lookahead`는 독립 투영의 예고 범위이며 기본 30초, 허용 0–3600초다. jump 전/후의 `[clock, clock+horizon]` 표시 행을 별도 기록한다. 이 값은 cactbot 설정이나 UI 동작을 보장하지 않는다.                                                                                                                                                                 |
| raw 안전 검사    | 각 train pull의 아군·melee·동시 instance까지 모든 raw cast/begincast를 재생한다. 대응 근거는 sync 선택 후 검사에만 사용한다. 같은 시각의 서로 다른 분기 신호는 원본 순서로 선택하지 않고 실패로 보고한다.                                                                                                                                                |
| 후보 거부        | 오매칭·모호함·잘못된 jump·window 미검출·누락·대응 미확정 또는 SPEC 검증 실패가 있으면 분기를 제외하고 공통 행의 전환만 한 번 별도 검사한다. 이 후보도 통과하지 못하면 P5 공통 초안과 `extensions.accepted=false`·이유·가능한 실패 집계를 보존한다.                                                                                                       |
| holdout          | 생성에 사용하지 않는다. 관측한 selector와 이후 경로를 별도로 대응하며 미지 selector·새 후속 경로·window 밖 도착은 실패 진단을 저장한다. 선택하지 않은 형제 경로도 실제 sync가 매칭하면 실패한다.                                                                                                                                                         |
| forcejump        | 생성하지 않는다. exit sync와 forcejump 우선순위에 의존하지 않는다. 기존 수동 작성 forcejump의 P6 재생 정책은 유지한다.                                                                                                                                                                                                                                   |
| 결정성·저장      | 입력 순서를 고정하고 가상 시각으로 안정 정렬한다. syncOrder 비활성화를 사용하지 않는다. YAML·JSON·Markdown은 검증 후 새 파일에만 저장한다.                                                                                                                                                                                                               |

## 근거 필드

| 위치                                 | 의미                                                                                                   |
| ------------------------------------ | ------------------------------------------------------------------------------------------------------ |
| `extensions.branches[].paths`        | 경로 label·selector slot·전체 경로 slot·가상 진입·공통 합류 위치                                       |
| `extensions.phases[]`                | 전환 selector slot·원래 alignment slot·label·가상 진입                                                 |
| `slots[].samples`                    | 원본 file/event index, 실제 관측 시각·블록 진입 시각·블록 기준 상대시간                                |
| `slots[].clockTime`, `windowMs`      | 해당 행의 보정된 가상 시계 통계·최종 활성 before/after 범위                                            |
| `slots[].transitionClockRange`       | 분기 selector에 사용한 모든 선택 신호의 도착 범위                                                      |
| `alignmentSlots`, `alignmentBlocks`  | P5의 fight-relative 중앙값과 정렬 근거. 최종 가상 좌표와 구분한다.                                     |
| `extensions.checks[]`                | 원본별 재생 집계·실행 jump·jump 전/후 예고 목록                                                        |
| `extensions.rejectedBranchCandidate` | 분기를 제외한 공통 페이즈 후보만 채택했을 때, 거부한 분기 확장의 이유·실패 근거                        |
| `pulls[].replay.jumps`, `previews`   | `replay` 명령의 실제 독립 시계 진입과 예고 투영. `entryIndices`는 현재 YAML entry index다.             |
| `validation`                         | 생성 후보의 train 독립 재생 통과를 기록한다. `status`는 편집 가능한 `draft`, parser/runtime은 false다. |

## 검증 범위와 제한

| 검사                   | 범위                                                                                                                                                                                                                                                                                                                      |
| ---------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Rust 회귀              | 두 판별 경로와 공유 중간 신호, 공통 prefix, 진입·합류, 독립 holdout, 미지 selector·새 후속, 선택 전/후 wipe, corrected window 경계, raw 경쟁 신호 거부, 결정성·lookahead 설정·안전 저장. 단일 보스 anchor와 같은 시각의 helper 순서를 바꾼 반복 회차에서 양방향 대응·시전 시작/완료·원본 index·wipe 접미부 보존 검사 포함 |
| 예고 투영              | 분기 선택 후 해당 경로의 후속 행이 보이고 다른 경로의 후속 행은 보이지 않는지 검사한다.                                                                                                                                                                                                                                   |
| CLI 평가               | 기존 고정 manifest와 train/holdout 평가 스크립트를 그대로 사용할 수 있다. 실패한 train-only 그룹과 미실행 holdout을 구분한다.                                                                                                                                                                                             |
| 실제 parser/runtime·UI | 미실행. 연구의 공식 검증 코드 제외 방침과 SPEC parser gate가 미결정인 상태를 유지한다.                                                                                                                                                                                                                                    |
| 미지원 생성            | 공통 보스 경계로 나눌 수 없는 분기, 한 dispatch로 구분되지 않는 중첩 선택, 미관측 출구, 무한 반복 압축, 관측 관계 없는 encounter 연결                                                                                                                                                                                     |
| 관측 상한              | 표본 밖의 긴 전투·다른 lookahead·새 경로는 별도 replay가 필요하다. 내부 투영으로 실제 UI 표시나 범용 전투 성능을 주장하지 않는다.                                                                                                                                                                                         |

## 2026-10-06 검증 결과

| 검사                                                          | 결과                                                                                                                    |
| ------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------- |
| `cargo test`                                                  | 157개 통과. 동시 anchor/helper 순서를 바꾼 반복 회차·양방향 대응·wipe 접미부 검사 포함                                  |
| `cargo clippy --all-targets -- -D warnings`                   | 통과                                                                                                                    |
| `cargo fmt --check`, `git diff --check`                       | 통과                                                                                                                    |
| `cargo build --release`                                       | 통과. Rust 1.99.0                                                                                                       |
| `nu scripts/test_evaluate_replay.nu target/release/btimeline` | 통과. Nushell 0.116.1. 안전 검사에서 거부한 분기·전환 때문에 train이 실패할 때 holdout 부재로 숨기지 않는 CLI 회귀 포함 |
| `nu scripts/test_freeze_evaluation_inputs.nu`                 | 통과                                                                                                                    |
| 고정 224개 입력 평가                                          | 기본·중복 후보 제외 평가의 4개 그룹씩 총 8개 모두 train/holdout 통과. 평가 스크립트 종료 코드 0                         |
| 실제 parser/runtime 표시                                      | 미실행                                                                                                                  |

고정 224개 입력을 `nu scripts/evaluate_replay.nu docs/evaluation-inputs.csv target/p7-single-anchor-evaluation --binary target/release/btimeline`로 평가했다. 기본·중복 후보 제외 평가의 상세 결과는 `target/p7-single-anchor-evaluation/evaluation.json`·`evaluation.md`와 각 그룹의 생성·재생 report에 보존한다. 재실행은 새 출력 디렉터리를 사용한다.

| 재현 근거           | 값                                                                 |
| ------------------- | ------------------------------------------------------------------ |
| manifest SHA-256    | `5c4b88f769925be4d83f1401ffb60e5e2a5686f1f73070538a6908e8633b02c3` |
| 실행 binary SHA-256 | `f6c0d75bd760256d5b0c6ed578fbb0e9700cded5af716f056bc8c2ce092c62bc` |
| 예고 범위           | 독립 투영 30초                                                     |

| 기본 평가              | train / holdout | 채택한 페이즈 전환 | train / holdout match | train / holdout jump | holdout 결과        |
| ---------------------- | --------------: | -----------------: | --------------------: | -------------------: | ------------------- |
| Clyteum · 4551/10      |          12 / 3 |                  2 |              288 / 72 |               24 / 6 | 대표 관측 범위 통과 |
| Dancing Mad · 1085/100 |         40 / 10 |                  1 |            1160 / 272 |               19 / 5 | 대표 관측 범위 통과 |
| R12S · 104/101         |          36 / 9 |                  0 |     P5 공통 초안 유지 |                0 / 0 | 통과                |
| R12S · 105/101         |         91 / 23 |                  0 |     P5 공통 초안 유지 |                0 / 0 | 통과                |

Clyteum·Dancing Mad에서 분기 확장 후보는 거부했고 공통 페이즈 전환만 채택했다. 두 전투의 train/holdout 오매칭·모호한 활성 매칭·잘못된 jump·관측 의존성 위반·window 미검출·누락 지표는 0이다. jump마다 전/후 두 예고 목록도 기록했다. 이 결과는 SPEC 초안의 대표 관측 범위 검사이며 전체 기믹 복원·첫 보스 포함·실제 UI 표시에 대한 주장이 아니다. 실제 분기 블록의 양·음성 검사는 미지 ID/이름의 합성 사례로 수행했다.

| 실제 전환             | selector slot / ability ID (10진수) | 보정 시계 min / max (ms) | window before / after (초) | 표본 |
| --------------------- | ----------------------------------- | -----------------------: | -------------------------: | ---: |
| Clyteum `phase-0`     | 0 / 48884                           |          346162 / 740457 |               90.8 / 303.6 |   12 |
| Clyteum `phase-1`     | 16 / 50313                          |        2275986 / 2371468 |                35.8 / 59.8 |   12 |
| Dancing Mad `phase-0` | 44 / 47764                          |          744357 / 758607 |                  8.7 / 5.7 |   19 |

위 ID는 실행 결과이며 생성기의 전투 조건문에 사용하지 않는다. Clyteum의 긴 window도 원본 전체 재생에서 다른 신호로 활성화되지 않았음을 검사했다.

| Dancing Mad 대응 보완 | 결과                                                                                                                                                                                                |
| --------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 원인                  | `Yj8wrgHzZWBDmvc7` fight 3과 train fight 20에서 Hyperdrive와 같은 시각의 helper 원본 순서가 달라, 뒤의 Grand Cross가 서로 다른 반복 회차 구간에 정렬됐다.                                           |
| 수정                  | 보스 완료 anchor가 하나인 동시 묶음의 정렬 순서를 고정했다. 원본 순서·시각·instance·양방향 합의·wipe 잘림 판정은 유지한다. 다중 anchor 묶음은 이번 순서 정규화에서 제외한다.                        |
| 기존 초안 재생        | `entryIndex: 47`의 Grand Cross가 `matched`, `expected: true`, `expectedEventIndices: [37976]`으로 복구됐다. 기존 시각·window를 그대로 사용하며 시계 오차는 +8 ms, 해당 pull의 `unverified`는 0이다. |
| 재생성·holdout        | Grand Cross의 대응 근거가 확인됐다. 추가 공통 cast도 보존되어 train/holdout match는 1160/272다. 시각상 sync 매칭만으로 대응 근거를 확정하지 않는다.                                                 |
| 이전 기록             | 보완 전 실패 산출물은 `target/p7-evaluation-final`에 보존한다.                                                                                                                                      |
