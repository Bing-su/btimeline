# 입력 → cactbot 전체 경로 검증

검증일: 2026-10-07. **확인된 세 결함을 보완했고, 생성된 원본 출력이 cactbot 소비자 검증을 통과했다.** 게임·ACT 캡처·실제 브라우저 표시 검증은 별도이며 자동으로 `runtime_compatible` 상태를 부여하지 않는다.

## 보완 후 결과

| 보완                 | 동작 / 회귀 근거                                                                                                                                                                                |
| -------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| network field 정규식 | ID·이름을 앵커 없이 출력하고, 내부 replay는 소비자처럼 전체 로그에 field 정규식을 삽입한다. 스칼라 대안의 우선순위, 배열의 묶음, 접두 이름과 필드 안 `^…$`의 소비자 의미도 회귀로 검사한다.     |
| 최초 진입            | 모든 단일·다중·P7/P8 YAML에 숨긴 `0.0 "--sync--" InCombat { inGameCombat: "1" } window 0,1` 행을 하나만 생성한다.                                                                               |
| 원본 대응 보존       | lifecycle 행은 cast 슬롯·P7/P8 대응에서 제외하고 매번 재생성한다. 실제 FFLogs event index를 만들지 않으며 내부 replay는 `combatStartEntry`로 시작 가정을 기록한다.                              |
| window 경계          | 하한 포함·상한 제외로 내부 검사를 바꿨다. P7/P8의 계산된 상한은 마지막 관측값보다 크게 반올림한다. 기본 window의 상한에 놓인 다중 로그 sample도 안전하지 않은 것으로 처리한다.                  |
| 소비자 검증          | 실제 `InCombat` 네트워크 채널에 시작 신호를 전달한다. 원본 출력의 검사에서 정규식 패치·강제 `SyncTo` 호출을 제거했다. 같은 엔진을 reset하고 전체 raw 신호를 다시 재생해 대응의 일치를 검사한다. |

`InCombat`은 FFLogs 원본에서 관측한 이벤트가 아니라 fight start에 대응한다고 가정한 라이프사이클 신호다. 생성 YAML의 note와 소비자 검증 범위에 이 가정을 명시했다. 실제 ACT 캡처에서 시작 신호 시각과 FFLogs fight start의 차이를 별도로 확인해야 한다.

| 검사                            | 보완 후 결과                                                                                           |
| ------------------------------- | ------------------------------------------------------------------------------------------------------ |
| Rust 테스트                     | 177개 통과. 기존 175개와 최초 진입·필드 경계 회귀 2개; 기존 window 경계 회귀의 상한 기대도 변경        |
| build / check / Clippy / format | release 빌드, `cargo check --all-targets`, Clippy `-D warnings`, nightly fmt, diff 검사 통과           |
| 평가 도구 회귀                  | `test_evaluate_replay.nu`, `test_freeze_evaluation_inputs.nu` 통과                                     |
| 고정 224개 train/holdout        | 기본·중복 후보 제외 8개 그룹 모두 통과                                                                 |
| 추가 입력                       | R11S 44개, 실제 단일 raid·dungeon 경로 통과                                                            |
| 실제 cactbot parser             | 타임라인 15개, 오류 0                                                                                  |
| 원본 출력의 정규식·엔진         | 기대한 cast 동기화 11,305 / 11,305, 누락·추가 매칭 0                                                   |
| 최초 진입·reset·재진입          | 462개 pull 사례 모두 통과. 각 사례를 동일 엔진에서 최초 진입·재진입 두 번 실행                         |
| 합성 P7/P8                      | P7 분기 8회 jump, P8 두 반복 사례 각각 16회 계속/출구 jump. 실제 로그의 P8 압축 성공을 뜻하지 않음     |
| window 경계                     | 10초 행에서 7.5초·10초 매칭, 12.5초 비매칭으로 내부·소비자 결과 일치                                   |
| 실제 HTML view                  | 15개 원본의 시작 신호·reset·재진입 DOM 검사 통과. 화면 밖의 첫 event는 진입 직후 표시되지 않을 수 있음 |
| 미실행                          | ACT 원본·게임/OverlayPlugin·브라우저 레이아웃/애니메이션·다른 언어 치환·104→105 연결                   |

| 보완 후 실행 근거     | 값                                                                                             |
| --------------------- | ---------------------------------------------------------------------------------------------- |
| 코드                  | 기준 commit `178f3afa95865f9f0fd2fe6371a72da27c2e28aa`에 이번 미커밋 보완 적용                 |
| binary SHA-256        | `d0d203d02464c3259068a5d0491f05784c528f807916e92d0dcd75a33bc7d1ad`                             |
| cactbot               | `0.37.5`, commit `d8b89c1c376f8cad1b3765b5929f326ee5d7ca4e`                                    |
| 산출물                | `target/e2e-cactbot-fixed-20261007/evaluation.json`, `cactbot.json`, 그룹별 YAML·텍스트·report |
| 소비자 검증 종료 코드 | **0**                                                                                          |

아래의 초기 검사 기록과 구분해 보완 후 결과를 재현하려면 새 출력 디렉터리에서 같은 평가·소비자 명령을 실행한다. 추가 실제 입력도 검증하려면 먼저 같은 폴더에 산출물을 준비한다. 예를 들어 이번 추가 입력은 다음과 같이 생성·재생했다.

```sh
btimeline generate logs/r11s -o OUT/supplemental/r11s/draft.yaml
btimeline replay OUT/supplemental/r11s/draft.yaml logs/r11s -o OUT/supplemental/r11s/train.replay.json
btimeline generate logs/r12s/4kz6WLqPGAJ2p8xg_1.json -o OUT/supplemental/single-real/draft.yaml
btimeline replay OUT/supplemental/single-real/draft.yaml logs/r12s/4kz6WLqPGAJ2p8xg_1.json -o OUT/supplemental/single-real/train.replay.json
btimeline generate logs/clyteum/AVxmgdNytFPBwLRH_31.json --mode dungeon -o OUT/supplemental/dungeon-real/draft.yaml
btimeline replay OUT/supplemental/dungeon-real/draft.yaml logs/clyteum/AVxmgdNytFPBwLRH_31.json -o OUT/supplemental/dungeon-real/train.replay.json
```

이번 소비자 실행은 아래의 초기 실행 명령에서 출력 루트를 `target/e2e-cactbot-fixed-20261007`로 바꿔 수행했다. 검증 스크립트의 기본 실행은 8개 고정 산출물과 4개 합성 산출물을 검사하고, 추가 폴더 인자를 주면 위 실제 경로까지 검사한다. 재실행에는 새 출력 디렉터리를 사용한다.

## 보스 구간 reset과 스칼라 대안 추가 보완

| 보완                       | 회귀 근거                                                                                                                                                                                                                                                                   |
| -------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 여러 dungeon/alliance 보스 | 겹치는 boss span은 합치고, 서로 다른 구간의 시간을 1000초 격자로 분리한다. 구간 첫 안전한 cast의 window는 0초까지 열린다. 기존 P7/P8 label·jump와 원본 index는 보존한다.                                                                                                    |
| 넓어진 진입 window         | 전체 raw train cast를 다시 검사하고 `bossSections`·`bossSectionChecks`에 좌표 이동과 검사 결과를 기록한다. 첫 보스의 표시 행이 모두 제외되어도 이후 구간 진입을 분리한다.                                                                                                   |
| 스칼라 대안                | `source: "Boss\|Unused"`가 `BossExtra`에서 잘못 동기화하는 소비자 동작을 내부 replay도 감지한다. 스칼라·원소 하나짜리 배열은 그대로 삽입하고, 여러 원소의 배열은 묶는다.                                                                                                    |
| Rust 회귀                  | 193개 테스트, Clippy, nightly fmt와 diff 검사 통과                                                                                                                                                                                                                          |
| 실제 소비자 추가 검사      | 단일 Clyteum dungeon의 87개와 다중 Clyteum alliance 15개 입력의 300개 대응을 지역 완료·재진입 신호와 함께 검사했다. 합성 5개를 합친 7개 타임라인·34개 pull에서 523/523 대응, parser 오류 0으로 통과했다. 합성 P8에는 두 번째 보스를 추가해 반복 label·jump 보존도 검사했다. |

추가 소비자 검사의 `7DE`와 보스별 `InCombat`은 FFLogs boss span 경계에서 모델링한 신호다. 원본 ACT에서 관측한 신호가 아니며, 게임·ACT 캡처 검증 범위는 확대하지 않는다. 기존 위쪽 산출물·binary 해시는 앞선 실행 기록이다. 추가 결과는 `/tmp/btimeline-final-consumer/cactbot.json`에 기록했다.

## 초기 실패 기록

이 아래는 보완 전 binary와 소비자 실행의 기록이다. 당시 생성 결과는 내부 검증을 통과했지만, 네트워크 sync 정규식과 최초 진입 문제 때문에 편집 없이 동작하지 않았다.

이번 요청의 명시적인 호환성 검증 범위에서 로컬 cactbot parser/controller/engine/HTML view를 사용했다. 공식 타임라인·트리거를 생성 입력, 학습 자료, 기믹 정답으로 사용하지 않았다. 기존 연구·평가 문서의 parser 미실행 기록은 당시 실행 결과다.

## 범위와 실행 근거

| 항목                  | 실행 범위 / 값                                                                                                                           |
| --------------------- | ---------------------------------------------------------------------------------------------------------------------------------------- |
| btimeline commit      | `178f3afa95865f9f0fd2fe6371a72da27c2e28aa`                                                                                               |
| 실행 binary SHA-256   | `5765c2c83f457a30487edb10655d9d397d5220b6d2ce2ac3a29270ef7573c5dc`                                                                       |
| 고정 manifest SHA-256 | `5c4b88f769925be4d83f1401ffb60e5e2a5686f1f73070538a6908e8633b02c3`                                                                       |
| cactbot               | 로컬 `0.37.5`, commit `d8b89c1c376f8cad1b3765b5929f326ee5d7ca4e`                                                                         |
| Node / Nushell        | `v24.21.0` / `0.116.1`                                                                                                                   |
| 실제 입력             | `logs/`의 268개 수집 JSON, 5개 encounter/difficulty 그룹을 `inspect-inputs`로 검사                                                       |
| 고정 평가             | 기존 224개 train/holdout 분할, 기본·중복 후보 제외 8개 그룹                                                                              |
| 추가 실제 입력        | R11S 44개 전체 생성·재생. 별도 holdout 평가가 아닌 추가 입력 경로 검사                                                                   |
| 추가 생성 모드        | 실제 R12S 단일 파일 raid, 실제 Clyteum 단일 파일 dungeon                                                                                 |
| 합성 입력             | P7 두 분기, P8 단일 cast·boss/helper 반복 각각의 clear/wipe·별도 holdout, 단일 파일                                                      |
| 소비자 실행           | 변환 텍스트 15개, 총 462회 pull 재생. 중복 제외 평가·추가 단일 입력·합성 재생을 포함한 실행 수이며 서로 다른 실제 입력은 268개           |
| 신호                  | 모든 raw cast/begincast의 ID·actor 이름·순서를 canonical `21`/`20` 네트워크 로그로 재구성. 선택에서 제외된 cast도 투입                   |
| 시계 / 표시           | 실제 cactbot 예약 콜백을 가상 시계로 실행. 실제 `HTMLTimelineUI`·`TimerBar`는 jsdom DOM으로 검사                                         |
| 미실행                | FFLogs API 신규 수집, 원본 ACT 캡처, 게임/OverlayPlugin, 실제 브라우저 레이아웃·애니메이션, 다른 언어의 이름 치환, 미관측 encounter 연결 |

FFLogs 수집 JSON에는 ACT의 모든 필드가 없으므로 canonical 로그의 미관측 필드는 자리 채움 값이다. 생성 결과가 사용하는 ID/source 조건에 대한 소비자 정규식 검사에는 충분하지만, 이 실행은 실전 ACT 캡처 검증이 아니다.

## 경로별 판정

| 게이트                             | 결과                                    | 근거                                                                                                                 |
| ---------------------------------- | --------------------------------------- | -------------------------------------------------------------------------------------------------------------------- |
| 입력 참조·완료 상태·그룹 검사      | 통과                                    | 실제 268개 `inspect-inputs`; 고정 224개 해시·분할 재검사                                                             |
| Rust 회귀                          | 통과                                    | `cargo test`: 175개, Clippy, nightly fmt, release 빌드                                                               |
| 평가 도구 회귀                     | 통과                                    | `test_evaluate_replay.nu`, `test_freeze_evaluation_inputs.nu`                                                        |
| 생성 → YAML schema/semantic → 변환 | 통과                                    | 15개 산출물을 `validate`·`convert`로 처리, 소비자 event 수와 YAML event 수 일치                                      |
| 독립 원본 리플레이                 | 통과                                    | 8개 고정 그룹 train/holdout, R11S·실제 단일 모드·합성 경로                                                           |
| 실제 cactbot parser                | 통과                                    | 변환 텍스트 15개, parser 오류 0                                                                                      |
| 소비자 sync 정규식                 | **실패**                                | 내부에서 맞았던 11,305개 대응 중 실제 정규식 매칭 0                                                                  |
| 최초 진입·시계 시작                | **실패**                                | 원본 출력의 462회 재생 모두 시작 0, 타이머 표시 0                                                                    |
| sync window 경계                   | **불일치**                              | 실제 소비자는 상한에서 sync를 제거하지만 내부 replay는 양 끝을 포함                                                  |
| reset / 독립 재진입                | 제한적 확인                             | 소비자 Stop이 시계·타이머를 정리. 진단용 강제 시작 후 HTML reset·재진입 검사; 원본은 최초 진입부터 실패              |
| P7 분기·P8 반복 / 표시             | 제한적 확인                             | 앵커 제거·강제 시작한 메모리상의 진단 복사본에서 대응·jump·표시 callback 확인. 원본 산출물의 동작 성공으로 세지 않음 |
| 최종 상태                          | **실패 / runtime_compatible 부여 불가** | parser 성공과 동기화·시계 실행 성공은 서로 다름                                                                      |

## 확인된 결함

| 우선순위 | 위치                                                                                      | 재현 / 원인                                                                                                                                                                                                                                                                            | 필요한 보완                                                                                        |
| -------- | ----------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------- |
| P1       | `src/generate/draft.rs:270`, `multi.rs:287`, `phase.rs:416`; `src/timeline/replay.rs:179` | 생성기는 network field를 `^B4D7$`, `^Lindwurm$`처럼 독립 문자열 정규식으로 만든다. 내부 replay는 각 필드 문자열을 따로 검사한다. cactbot은 이 값을 전체 로그 정규식 중간에 삽입하므로 `^`/`$`가 전체 로그 시작/끝을 요구해 매칭이 불가능하다. P8도 기존 생성 field를 재사용해 영향받음 | network field의 소비자 의미와 내부 replay 의미를 일치시키고, 실제 소비자 매칭을 회귀 게이트로 유지 |
| P1       | 생성기의 첫 event/진입 정책; `src/timeline/replay.rs:308`                                 | 내부 replay는 시계 0에서 즉시 시간 진행을 시작하지만 cactbot은 `Stop()` 후 0초에 활성화된 sync만 받는다. 전투 진입 `SetInCombat(true)`만으로 시계가 시작되지 않는다. 실제 다중 로그 5개 그룹 모두 0초 활성 sync가 없다                                                                 | 관측한 시작 기준에 맞는 최초 진입 sync를 생성하고, 정지 상태에서 진입하는 소비자 검사를 추가       |
| P2       | `src/timeline/replay.rs:455`; cactbot `timeline.ts:350`                                   | 내부 replay는 `clock <= windowEnd`를 허용한다. 실제 소비자는 예약 callback에서 `sync.end <= fightNow`인 sync를 제거한다. 10초 행의 기본 window에서 7.5초·10초는 매칭하지만 정확히 12.5초는 매칭하지 않는다                                                                             | 내부 시계·window 경계 정책을 소비자에 맞추고 실제 예약 callback을 사용하는 경계 회귀를 유지        |

대표 R12S 104 출력은 다음과 같다.

```text
16.0 "The Fixer" Ability { id: "^B4D7$", source: "^Lindwurm$" }
```

실제 cactbot parser는 오류 없이 다음 정규식을 만든다.

```text
/^2[12]\|[^|]*\|[^|]*\|^Lindwurm$\|^B4D7$\|/i
```

`21|2026-10-07T00:00:00.0000000+00:00|40000001|Lindwurm|B4D7|...`는 이 정규식과 맞지 않는다. 앵커를 제거해 이 문제만 우회해도 첫 행의 window는 `[13.5, 18.5]`초이므로 정지 시점 0초에서 활성화되지 않는다. 네트워크 로그가 도착해도 시계가 시작되지 않고, 재진입 때도 같은 상태다.

window 경계 검사는 위 두 결함의 영향을 제거한 별도 소비자 fixture로 실행했다. 내부에서 양 끝 포함으로 통과시킬 수 있는 정확한 상한이 소비자에서는 누락된다. 이 불일치를 기존 실제 입력에서 추가 실패로 재현한 것은 아니며, 현재 실제 입력의 동작을 막는 직접 원인은 앞의 두 P1 결함이다. 세 경계의 실제 실행 결과는 JSON의 `boundaryChecks`에 있다.

## 원인 분리 결과

다음은 **메모리에만 적용한 진단용 변경**이다. 출력 YAML/텍스트와 Rust 생성 코드를 고치지 않았다. 앵커만 제거하는 실행과, 추가로 소비자 `SyncTo(0, ...)`를 호출하는 실행을 구분했다. 후자는 일반 소비자가 자동으로 수행하는 동작이 아니다.

| 실제 기본 그룹       | 입력 수 | event / 활성 sync | 내부 대응 수 | 앵커 제거만으로 시작 | 앵커 제거 + 강제 시작 대응 / jump |
| -------------------- | ------: | ----------------: | -----------: | -------------------: | --------------------------------: |
| Clyteum 4551/10      |      15 |           29 / 24 |          360 |                    0 |                          360 / 30 |
| Dancing Mad 1085/100 |      50 |           76 / 55 |        1,432 |                    0 |                        1,432 / 24 |
| R12S 104/101         |      45 |           32 / 32 |        1,077 |                    0 |                         1,077 / 0 |
| R12S 105/101         |     114 |           38 / 38 |        2,337 |                    0 |                         2,337 / 0 |
| 추가 R11S 103/101    |      44 |           58 / 50 |        1,510 |                    0 |                         1,510 / 0 |

강제 시작한 진단 실행에서 15개 타임라인 모두 기대 대응 누락·추가 매칭은 0이었다. 합성 P7은 8회 jump, 합성 P8 두 사례는 각각 16회 계속/출구 jump를 수행했다. 합성 입력의 첫 완료는 1–2초여서 앵커 제거만으로도 시작되므로, 실제 입력의 최초 진입 결함을 합성 테스트만으로 발견하지 못한다.

실제 HTML view에서 원본 출력은 15개 모두 시작 시 타이머 DOM이 비어 있었다. 진단용 강제 시작 시 R12S 104의 `The Fixer`, `unknown_b7c4`와 105의 `Arcadia Aflame`, `unknown_b4d9`, `Replication`이 타이머로 생성됐고 reset 후 제거됐다. Clyteum/Dancing Mad의 첫 출력은 초기 30초 밖이므로 강제 시작 직후에도 비어 있을 수 있다. jump 전/후 실제 UI callback의 표시 이름은 JSON의 `jumpDetails`에 보존한다. jsdom 결과로 실제 브라우저 배치·애니메이션 성공을 주장하지 않는다.

실제 P8 반복 후보는 여전히 모두 압축을 거부하고 유한 초안을 유지했다. 합성 압축의 성공과 실제 입력의 압축 성공은 구분한다. 내부 replay는 표현한 행만 검증하며 전체 기믹 복원도 보장하지 않는다. 추가 입력의 미표현 cast 수는 JSON에 기록했다.

## 재현과 산출물

기존 출력은 덮어쓰지 않으므로 새 평가 디렉터리를 사용한다. cactbot checkout에 기존 `ts-node`, `js-yaml`, `jsdom-global` 의존성이 설치되어 있어야 한다.

```sh
cargo build --release
nu scripts/evaluate_replay.nu docs/evaluation-inputs.csv target/cactbot-new-run --binary target/release/btimeline
TS_NODE_TRANSPILE_ONLY=true \
TS_NODE_PROJECT=/home/ks2515/workspace/cactbot/tsconfig.json \
node --loader /home/ks2515/workspace/cactbot/node_modules/ts-node/esm.mjs \
  scripts/verify_cactbot.mjs target/cactbot-new-run /home/ks2515/workspace/cactbot
```

검증 스크립트는 기본 8개 산출물과 합성 4개를 검사한다. 이번 실행의 추가 실제 경로는 아래 명령으로 포함했다. 추가 폴더는 생성된 `draft.yaml`, `draft.report.json`, `train.replay.json`을 갖는다.

```sh
TS_NODE_TRANSPILE_ONLY=true \
TS_NODE_PROJECT=/home/ks2515/workspace/cactbot/tsconfig.json \
node --loader /home/ks2515/workspace/cactbot/node_modules/ts-node/esm.mjs \
  scripts/verify_cactbot.mjs target/e2e-cactbot-20261007 \
  /home/ks2515/workspace/cactbot target/release/btimeline \
  target/e2e-cactbot-20261007/supplemental/r11s \
  target/e2e-cactbot-20261007/supplemental/single-real \
  target/e2e-cactbot-20261007/supplemental/dungeon-real
```

| 산출물                                                         | 내용                                                                                         |
| -------------------------------------------------------------- | -------------------------------------------------------------------------------------------- |
| `target/e2e-cactbot-20261007/all-inputs.json`                  | 실제 268개 입력 그룹 검사                                                                    |
| `target/e2e-cactbot-20261007/evaluation.json`, `evaluation.md` | 고정 입력·해시·train/holdout 내부 결과                                                       |
| `target/e2e-cactbot-20261007/cactbot.json`                     | 실제 parser 오류, 원본/원인 분리 4종 소비자 재생, 대응 수, jump 전후 표시, HTML reset·재진입 |
| 각 그룹 `draft.txt`                                            | CLI가 변환한 원본 cactbot 텍스트                                                             |
| `scripts/verify_cactbot.mjs`                                   | 실패를 종료 코드 1로 반환하는 소비자 검증 게이트                                             |

초기 소비자 검증은 종료 코드 **1**이며, 결함을 기록한 실패 결과다. 초기 검증 당시에는 생성기·변환기 동작 변경을 적용하지 않았다. 현재 검증 스크립트는 위 보완 후 원본 실행과 경계·재진입 검사를 수행한다.
