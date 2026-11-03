# Clyteum: 참조 타임라인 없이 작성

2026-11-02~03. [timeline-author skill](../.agents/skills/timeline-author/SKILL.md)로 Eye of the Scorpion·Chort·Malphas 세 보스를 작성했다. cactbot 전투 타임라인·기믹 가이드·공략 영상은 사용하지 않았다. 고정 체크아웃 `e79ea7f16093be5b80cda0e2d5bc8eb899d6fb4b`에는 Clyteum과 산도리아의 전투 타임라인 파일이 없었다. Zone ID 조회만 메타데이터로 사용했다.

기존 저장소의 Clyteum 감사 문서는 자료 위치·기존 한계 확인에 읽었으며 사전 지식이 전혀 없는 블라인드 실험은 아니다. 기존 자동 생성 KDL을 시간표 입력이나 템플릿으로 사용하지 않았고, 아래 일정·ID 대안·반복은 원본 cast·actor·HP 관측에서 작성했다.

| 항목         | 내용                                                                                                                                      |
| ------------ | ----------------------------------------------------------------------------------------------------------------------------------------- |
| KDL          | [clyteum.kdl](clyteum.kdl), Eye 22·Chort 21·Malphas 20개, 총 63개 이벤트와 보스별 loop 3개                                                |
| 후보 SHA-256 | `69c5639aabdfa9261e13d7f4f69b4ee8cbb4c27759bd2cc3f21c47420160afc3`                                                                        |
| 개발         | [개발 계획](../tests/fixtures/authored/clyteum-development-plan.json)의 31개 파일·해시. 기존 조사에 사용된 자료이므로 독립 근거가 아님    |
| 보류         | [보류 계획](../tests/fixtures/authored/clyteum-holdout-plan.json)의 미사용 보고서 4개·pull 6개. 후보·입력·판정 조건을 내용 확인 전에 고정 |
| 실용성 기준  | 주요 기믹의 첫 관측 5초 전 조회, 허용 오차 1.5초. 보스 진입 자체의 사전 예고·개별 위치·대상·모든 후속 패킷은 범위 밖                      |

## 원본 해석

| 결정           | 원본 근거·의도                                                                                                                                                                                                                               |
| -------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 보스 시도 분리 | 긴 파일에는 wipe 뒤 재도전이 섞임. 같은 이름의 main Boss actor 한 개·HP=0·보스 순서를 요구. HP가 75% 미만에서 90% 초과로 회복하면 재도전 또는 큰 회복일 수 있어 제외                                                                         |
| 관측 범위      | 보스 첫 대표 시전부터 해당 main actor HP=0까지. 구간 안의 모든 cast를 actor 이름의 Ability로 전달. 이동 중 이름 없는 시전은 이 검증 범위 밖이며 보스 구간 안의 이름 누락은 제외                                                              |
| Eye            | BF00을 원점으로 Beam 두 번·두 차례 missile 도입을 분리. 첫 Penetrator는 도입부에 있고 후반은 짧은 missile/Beam/Scanner 블록으로 반복                                                                                                         |
| Eye 주기       | `yf3A4pRTGQmHxa1q/8` 등의 긴 완료 시도와 후속 Ballistic의 문맥으로 반복 첫 Eyes를 구별. 중간 Eyes까지 같은 반복 경계로 세지 않음. 128.4초부터 약 59.2초                                                                                      |
| Eye ID 대안    | C3FF/C400은 이름과 후속 진행이 같아 같은 Beam 표시로 처리. 방향·안전 지점은 로그 이름만으로 확정하지 않음                                                                                                                                    |
| Chort          | 첫 Mortifying 튜토리얼·복합 두 번째 진행·짧은 후반 블록을 분리. BEE4–BEE7은 같은 이름과 후속 시각을 공유하므로 ID만으로 경로를 나누지 않음                                                                                                   |
| Chort 주기     | 후반 Evil Emission→Pressure→Vomit→Ripples→Bodyweight 묶음. 162.2초부터 약 71.5초. `pAzgtZK8aqjTJvGL/12`의 Chort `#15301→#16183` 간격과 후속 진행으로 지지. 이 파일은 Malphas 재도전 때문에 전체 평가에서는 제외하고 Chort 해석 근거로만 사용 |
| Malphas        | 첫 두 Puppet Strings는 도입부. 이후 Puppet Pair와 두 Puppet Mastery를 포함하는 더 큰 Pair→Pair 본문을 사용. 짧은 동반 시전의 재등장을 전체 반복으로 오인하지 않음                                                                            |
| Malphas 주기   | 관측된 Pair 경계 간격 표본의 대표값을 0.1초로 근사해 129.0초 사용. `NJV9Gnv8Zd2xQTDm/13`의 첫 두 Pair와 `yf3A4pRTGQmHxa1q/8`의 세 Pair가 실제 재반복을 지지                                                                                  |
| 표시 요약      | missile 대상별 패킷·Bodyweight 연속 타격·Puppet 관련 동반 발동은 대표 기믹으로 요약. Scrap Gargoyle/Byblos 개별 공격의 소속·위치 해석과 별도 표시는 아직 하지 않음                                                                           |
| 종료           | 기존과 같은 InCombat=0·wipe·ChangeZone 정책. FFLogs HP=0은 관측 범위이지 합성한 실제 ACT 종료 신호가 아님                                                                                                                                    |

예정 시각은 원본 발생의 대표값·구간 간격을 0.1초로 근사했다. Eyes·Scanner, Chort의 Vomit/Bodyweight/Ripples, Malphas의 Scrap/Pair/Void/Rubbish 중 구별 가능한 위치만 sync로 선택했다. 반복하는 같은 ID의 다른 발생과 window가 겹치지 않도록 분리했다.

## 개발 검증

보조 공격 후보를 기존 수집 로그 `AVxmgdNytFPBwLRH/31`에서 추적했다. Malphas의 Puppet Strings 뒤 Scrap Byblos의 `BF28` Metallic Miasma가 #14604, Scrap Gargoyle의 `BF26` Cast-Off Halo가 #14732–14737에 기록됐다. Puppet Pair 뒤에는 두 기술이 #15406–15413에 같은 밀리초로 기록됐다. 이 표본에서 Puppet 계열과의 선후 관계는 확인했지만, 개체별 패킷을 별개 예고로 표시할 근거와 다른 회차의 대응은 남아 있다.

| 검사              | 결과                                                                                                                                                    |
| ----------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 입력              | 31개 중 적격 23개 통과, 종료 미확인·재도전/회복 가능 자료 8개 제외                                                                                      |
| 구간 내 전체 cast | 41,364건                                                                                                                                                |
| Eye               | 표시 412/412, 조회 389/389, 최대 조회 오차 194ms                                                                                                        |
| Chort             | 표시 351/351, 조회 328/328, 최대 조회 오차 655ms                                                                                                        |
| Malphas           | 표시 447/447, 조회 424/424, 최대 조회 오차 547ms                                                                                                        |
| 구간 진입         | 모든 적격 자료에서 세 보스의 관측 진입 시각·순서 일치, 실행 진단 없음                                                                                   |
| 합성              | 각 보스에서 단독 시작·관측 없는 미래 회차·비전투 리셋 후 재시작, 늦은 대표 신호 +4/+5초 뒤 예고 복구 확인. 신호를 빼면 해당 지연을 복구하지 못함도 검사 |
| 검증 도구         | Mistwake와 공통 던전 평가기를 사용. 기존 Mistwake 보류의 모든 표본·제외·집계가 추출 전과 동일함을 대조                                                  |

## 고정 후보의 보류 평가

후보·해시·판정 기준을 `1ed237c`로 커밋한 뒤 2026-11-03에 새 로그를 평가했다. 4개 보고서의 6개 pull 중 Chort HP=0이 없는 `Cm37YbnrZRk8TFvt/390`을 제외하고, 3개 보고서의 적격 5개가 모두 통과했다. 후보 KDL은 수정하지 않았다.

| 보류 검사         | 결과                                                             |
| ----------------- | ---------------------------------------------------------------- |
| 범위 내 전체 cast | 9,893건                                                          |
| Eye               | 표시 97/97, 조회 92/92, 최대 조회 오차 274ms                     |
| Chort             | 표시 77/77, 조회 72/72, 최대 조회 오차 558ms                     |
| Malphas           | 표시 104/104, 조회 99/99, 최대 조회 오차 432ms                   |
| 합계              | 표시 278/278, 5초 전 조회 263/263. 보스 진입 오류·실행 진단 없음 |

보류의 가장 긴 관측은 Eye 158.210초·Chort 163.610초·Malphas 240.039초다. 긴 던전 기록이어도 각 보스의 전체 후반을 보장하지 않으므로 반복 근거를 별도로 정리했다.

| 반복 근거 | 개발                                                                                                                       | 보류                                                |
| --------- | -------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------- |
| Eye       | 주요 관측열 기준 완전 본문 4개·다음 경계 3개. 간격 59.133–59.312초                                                         | 첫 후반 블록 일부만 관측, 다음 경계 없음            |
| Chort     | 전체 파일 평가에서는 본문 1개·다음 경계 0개. 별도 부분 개발 근거 `pAzgtZK8aqjTJvGL/12`의 Chort가 다음 Evil Emission을 지지 | 후반 첫 진입까지만 관측, 다음 경계 없음             |
| Malphas   | 완전 본문 9개·다음 경계 7개. 간격 128.445–129.644초                                                                        | 완전 본문 4개·다음 경계 2개. 간격 128.899/129.285초 |

이 수는 선택한 주요 시전열의 본문·경계 수이며 모든 원본 패킷 재현의 판정은 아니다. Malphas의 실제 다음 회차까지 후보 수정 없이 확인했으며, Chort의 먼 미래 회차는 보류 자료로 확인하지 못했다. 미래 신호 없이 실행한 합성 회차와 실제 관측 회차를 혼합하지 않는다.

[검증 기록](clyteum.validation.json)에 후보·계획·입력 해시, 제외 사유·인덱스, 보스별 표본·재생/조회 오차와 `cycle_evidence`를 보존했다. `sample_columns`가 표본 열을 설명한다. 반복 첫 경계는 Eye의 Eyes→Ballistic 문맥, Chort의 세 번째 Ripples 뒤 Evil Emission, Malphas의 Puppet Pair로 식별했다. 개발·보류 간 보고서 중복과 주요 상대 시각·ID fingerprint를 검사했으며 모든 변형 업로드의 독립성까지 증명하지는 않는다.

```sh
cargo build
target/debug/btimeline validate timelines/clyteum.kdl
target/debug/btimeline evaluate --profile timelines/clyteum.evaluation.json tests/fixtures/authored/clyteum-development-plan.json -o out/clyteum-authoring/development-new.json
target/debug/btimeline evaluate --profile timelines/clyteum.evaluation.json tests/fixtures/authored/clyteum-holdout-plan.json -o out/clyteum-authoring/holdout-new.json
bun run --cwd scripts test
cargo test --test authored_clyteum
```

원본 경로·해시가 일치해야 하며 보고서는 새 경로로만 저장한다. 보류 결과를 보고 수정하면 해당 입력을 개발 자료로 전환한다. 실제 ACT·UI, 이동 중 종료 신호, 위치·대상 해석은 미검증이며 관측된 기믹 예고 범위만 평가한다.
