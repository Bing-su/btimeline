# 대표 그래프 말단 반복 검증

2026-11-02. 말단 반복의 첫 그래프 통합을 검사했다. 합성 자료의 KDL 단독 예고와 실제 자료의 생성 회귀를 구분한다. ACT·외부 실행기·게임 실행은 미검증이다.

## 합성 자료

`cargo test --test generate_loops`의 다음 사례는 생성기에 직접 FFLogs fixture를 넣고 검사한다. 입력은 테스트가 결정적으로 생성한다. 별도 검증 입력을 학습에 섞지 않으며 자동 보고서 보류 검증으로 취급하지 않는다.

| 사례                | 확인 결과                                                                                                                       |
| ------------------- | ------------------------------------------------------------------------------------------------------------------------------- |
| 긴 전멸 두 개       | 반복 파동 7개/11개와 내부 순서 변형을 한 `repeat`로 연결. raid·dungeon·alliance 지원                                            |
| opener 없는 보스    | 첫 반복 경계를 phase 진입으로 사용. 유한 경계는 loop로 대체                                                                     |
| 원본 보존           | 유한 경로와 `observed_waves` 참조의 합집합이 모든 실행 대상 발생을 보존. 마지막 부분 회차에 가짜 참조 없음                      |
| 입력 순서           | 역순 입력의 KDL과 바이트 단위 동일                                                                                              |
| 미관측 다음 회차    | evidence를 삭제하고 시작 관측만 공급. 학습 전투의 종료 이후인 20초 뒤에도 이벤트를 예고하며 ambiguous·unresolved·violation 없음 |
| 선택 후 반복 합류   | B→C / C→B를 선택한 뒤 공통 말단 반복으로 진행. raid·dungeon에서 선택 신호만으로 후속 일정과 미래 회차를 예고                    |
| 반복 시간 추정 실패 | 구조상 반복이어도 추정된 주기보다 본문이 길면 실패. KDL과 evidence 모두 저장하지 않음                                           |

## 실제 코퍼스 생성 회귀

모든 입력은 기존 학습용 파일이다. 이번에는 별도 holdout·원본 투영 재생·cactbot 비교를 수행하지 않았다. 두 묶음 모두 `generation_method=common-backbone-v1`, 생성 loop와 대표 `repeat`는 0개였다. 따라서 아래 성공은 새 반복 경로의 실제 자료 검증을 뜻하지 않는다.

| 묶음    | 명령                                                                                                                            | 결과 KDL SHA-256                                                                |
| ------- | ------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------- |
| R11S    | `btimeline generate logs/r11s/{4kt1bfqJn7QPCj29_2,c36H8h2aqXwkgtjR_3,fDhKygMQ3VCvZat8_2}.json --mode raid -o r11s.kdl`          | `2a11957af456818be2d7c4c44328629fbb1fbd9a01904681d13db5c4d2bf13cc` (187개 정의) |
| Clyteum | `btimeline generate logs/clyteum/{ZpHDQcCnfgBtaTPL_1,TwQYZydBhgLfn3bC_1,tpmrkL76NhXHMbZv_7}.json --mode dungeon -o clyteum.kdl` | `0280aa0adbeba0eea01cfbe6d069739b6bc294a718ba98549bd801983399ccb0` (65개 정의)  |

명령의 `{…}`는 zsh/bash의 brace expansion이다. 생성 시 출력 경로는 비어 있어야 한다.

## 후속: 반복 경로의 보고서 보류 검사

`repeat_holdouts_relearn_without_the_entire_report`의 14개 사례와 `a_holdout_cannot_supply_the_second_training_witness_for_a_repeat`를 추가했다. 각 보류 검사는 임시 학습 자료만으로 반복을 다시 발견하고 대표 그래프에 연결한다. 보류 관측은 시간·순서·지지 trace로 사용하지 않는다. 이는 최종 KDL 재생 검증과 별개다.

| 사례                                  | 결과                                                                                               |
| ------------------------------------- | -------------------------------------------------------------------------------------------------- |
| 같은 보고서의 두 전투                 | `zeta/1`, `zeta/2`를 함께 보류. 임시 모델의 모든 원본 참조에 zeta가 없음                           |
| 회차 안·회차 끝·진입 전 종료          | 다음 내부 단계나 다음 회차 경계 전에 끝난 경우만 `censored`                                        |
| 더 많은 회차                          | 학습 회차 수를 넘겨도 실제 학습 회차 순서로 대응. trace별 지지 수를 중복 계산하지 않음             |
| 기한 초과·내부 단계/경계 누락·새 순서 | `unresolved`. 실패 회차와 확인된 완전 회차 수를 기록                                               |
| 선택 후 공통 반복                     | raid·dungeon에서 유한 선택 경로를 먼저 확인한 뒤 회차를 검사                                       |
| 복합 파동·근접 순서                   | 검증 지원 범위 밖이므로 `unresolved`                                                               |
| 두 번째 학습 근거 부족                | 전체 입력으로는 반복을 생성하더라도 임시 모델에는 반복 없음. 보류 입력을 학습 근거로 보충하지 않음 |

### 실제 Mistwake 자료

명령: `btimeline generate logs/loop-mistwake/*.json --mode dungeon -o mistwake.kdl`.

| 결과                | 관측                                                                                                            |
| ------------------- | --------------------------------------------------------------------------------------------------------------- |
| 생성                | 94개 정의, `common-backbone-v1`. KDL SHA-256 `2a0a9b7c3daa4b797766f0e8243b24673d0e33382712e67ae2554301c0138df8` |
| 보류                | `dN2P37qHWt1mFD6R/13` 한 전투를 제외하고 아래 7개 파일로 임시 학습                                              |
| Griffin 임시 그래프 | 말단 반복은 발견했지만 유한 경로 `peers_complete=false`, 복합 파동 56개. 반복 검사는 `unresolved`               |
| 다른 보스           | Amdusias·Treno Catoblepas도 `unresolved`                                                                        |
| 판정 한계           | 처리 완료·생성 성공은 품질 통과를 뜻하지 않음. 새 실제 자료의 성공적인 반복 일반화·KDL 예고·ACT 동등성은 미확인 |

| 역할 | 입력 (`logs/loop-mistwake/`) | SHA-256                                                            |
| ---- | ---------------------------- | ------------------------------------------------------------------ |
| 학습 | `YQAawtmnD6FRVzPj_4.json`    | `1557e8b0e06a0cb2b1ed0a17998f157ea27bac2382f87170213cb6bdfc427b80` |
| 학습 | `YQAawtmnD6FRVzPj_78.json`   | `93b9707471ffffad105b2590a722c8f4c050619319ccfbb26aa4d393ea5b8b04` |
| 학습 | `YQAawtmnD6FRVzPj_79.json`   | `c55eba66ac5c3ea6ab6a944e34b3b108c2283798fda21467c324c9bacb7a9514` |
| 학습 | `YQAawtmnD6FRVzPj_80.json`   | `4ccb4542e91871ca9a41eab8db6986ae289c5c75a7b74411ea9b3375f497008b` |
| 학습 | `YQAawtmnD6FRVzPj_81.json`   | `82b31c085af0e76ff688032f850f25b150b3ab38f5e6e9c6883ed038de65050e` |
| 학습 | `YQAawtmnD6FRVzPj_82.json`   | `c4c4b1f86d4751d20c99bf4ffc2585d916a6242e45cd3bbc5fdbb55251d41ca9` |
| 학습 | `c6BadvCfAZyFqpQh_1.json`    | `79acfead5e46196ff07a76baa02098d3bebdb49f23f581f5f8c38413e2ca9f55` |
| 보류 | `dN2P37qHWt1mFD6R_13.json`   | `ce820880f5013845ab08f25f789c28c1a9a8335061cb9af02d12758a51d9370c` |

| 입력 파일                              | SHA-256                                                            |
| -------------------------------------- | ------------------------------------------------------------------ |
| `logs/r11s/4kt1bfqJn7QPCj29_2.json`    | `475d3009e53ff78bfc2cdf65021cbd7aeca45800110a4cd3116a3bb322e39bd4` |
| `logs/r11s/c36H8h2aqXwkgtjR_3.json`    | `d3f1f8a4d61c9486a0d98c31fd9e52ce30bd24f460cea3a20f90433fa8ebef61` |
| `logs/r11s/fDhKygMQ3VCvZat8_2.json`    | `d43354d67eb0c186c64d24889321c290ceb7d7b3e5fa01d6038a6d2e76ed66c3` |
| `logs/clyteum/ZpHDQcCnfgBtaTPL_1.json` | `785cf97ddbccc0c870c98485d1ef907d2780a58279cf962472b63f38efadd5fb` |
| `logs/clyteum/TwQYZydBhgLfn3bC_1.json` | `10f2d5bf0f58718d7874685ee391501e87429a2287d724c68fc4fadb5903744b` |
| `logs/clyteum/tpmrkL76NhXHMbZv_7.json` | `b36a637edccc1bb2659b43917cbfc1a852a266f4d847b8e03149ed4304c2b2df` |
