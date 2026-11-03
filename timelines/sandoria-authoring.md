# 산도리아: 두 번째 반향세계

2026-11-03. `timeline-author` skill로 원본 FFLogs의 cast·begincast·actor 관측을 해석했다. cactbot 전투 시간표·공략·영상은 사용하지 않았다. cactbot 고정 체크아웃 `e79ea7f16093be5b80cda0e2d5bc8eb899d6fb4b`의 공통 `resources/netlog_defs.ts`·`netregexes.ts`에서 WasDefeated의 `target` 필드만 확인했다. 이는 전투 진행을 얻은 근거가 아니며 실제 ACT에서 검증한 것도 아니다.

| 항목      | 범위                                                                                                                                                         |
| --------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| 산출물    | [sandoria.kdl](sandoria.kdl), 네 전투·131개 표시·Shijin 네 선택과 공통 합류                                                                                  |
| 첫 후보   | `784924f`의 121개 표시. 당시 SHA-256은 `71515ca84e56a05841ac4930c522369448e889807237510d43e316cd48345d6b`                                                    |
| 초기 개발 | [개발 계획](../tests/fixtures/authored/sandoria-development-plan.json)의 12개. 초기 수집의 PA 네 파일도 같은 시도의 복사본이므로 별도 표본으로 추가하지 않음 |
| 초기 보류 | [첫 보류 계획](../tests/fixtures/authored/sandoria-holdout-plan.json)의 16개: 9개 통과·7개 실패. 제외 없이 실패를 보존하고 모두 개발 자료로 전환             |
| 현재 개발 | [확장 개발 계획](../tests/fixtures/authored/sandoria-expanded-development-plan.json)의 28개. 첫 보류의 결과를 사용했으므로 이 후보의 독립 근거가 아님        |
| 현재 후보 | 확장 개발 계획과 새 보류 계획의 `timeline_sha256`·입력 해시·평가 조건으로 고정                                                                               |
| 평가 기준 | 주요 첫 발동 5초 전 조회, 오차 1.5초 이하. 첫 Shijin Quake/Stonega III는 선택 시작부터 발동까지 약 2.98초이므로 2초 전 조회로 분리                           |
| 진입      | 첫 대표 Ability에서 시작. 진입 자체와 진입 후 5초 미만 기술의 5초 전 예고는 평가하지 않음                                                                    |

경고 검토 이전 후보는 `8568a2e`, SHA-256 `4175ef50f2da90edbbb423cfea23befd5c8f51aeeb11c539a4aa3b766596c2ca`다. [당시 새 보류 계획](../tests/fixtures/authored/sandoria-fresh-holdout-plan.json)의 5개 보고서·16개 입력은 그 후보 고정 뒤 평가했고 모두 통과했다. 아래 경고 검토에서는 이 자료도 개발 근거로 전환했다.

최초 수집은 [다운로드 계획](../tests/fixtures/authored/sandoria-download-plan.json)의 `PAq8bNtxvXWG16QZ` 한 보고서에서 가져왔다. encounter 2067–2070은 각각 fight 68·70·72·73이다. 이 네 전투는 같은 보고서에서 수집했다.

## 원본 해석

`#`는 `raw.events`의 0부터 시작하는 인덱스다. [검증 기록](sandoria.validation.json)에 표본 인덱스·시각·ID·예고 간격·오차를 보존한다. 관측에서 직접 작성했고 자동 초안을 최종 근거처럼 덮어쓰지 않았다. 네 초기 전투의 `mine` 출력은 `observation_graphs`의 명령·해시로 추적하며 최종 KDL 실행에 필요하지 않다.

| 결정               | 원본 근거·의도                                                                                                                                                                                                                                                        |
| ------------------ | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Kirin 도입         | `PAq8bNtxvXWG16QZ/68` #1328 Stonega IV → #2727 Wrought Arms → #3813/#4716 Synchronized Strike → #5962/#6494 Crimson Riddle → #7696 Summon Shijin. 두 Crimson Riddle은 약 7초 떨어진 별개 표시                                                                         |
| Quake / Stonega    | PA/68 #10079 B079 시전 시작 → #10262/#10899 Stonega III. `MFCVjw7RNch6mxtZ/206` #10233 B07B 시작 → #10505와 후속 B07C Quake. 서로 다른 간격의 두 option                                                                                                               |
| Genbu 추가 경로    | `HnxFmz3CVZkNW1Xp/45` #8234 Moonbound Genbu AD84 시작 → #10877 Crimson Riddle → #12093 Wrought Arms. Riddle 자체를 선택 신호로 쓰면 다른 경로의 같은 기술과 충돌하므로 더 이른 Genbu 신호로 구별                                                                      |
| Eastwind 추가 경로 | `FqmhgXHZRdc6D7AK/14` #9598 AD82 → #10163 Crimson Riddle → #10938 Double Cast → #11642/#11652 Quake·Stonega III 동시 발동 → #12506 Wrought Arms. 동시 두 발동은 별도 표시이며 Riddle로 경로를 재선택하지 않음                                                         |
| 공통 합류          | 선택별로 두 번째 Wrought Arms를 미리 예고하고, 해당 AD91 시전 시작에서 합류해 3.5초 뒤 표시. Ability에서 합류하면 반올림으로 아직 실행되지 않은 option 표시를 취소할 수 있어 시전 시작을 사용                                                                         |
| Kirin 대안         | Striking Left/Right와 Wringer/Synchronized Strike 순서 대안은 `/`로 표현. 공통 Mighty Grip·Deadly Hold·Bury까지 이어지며 위치·안전 지점을 추정하지 않음                                                                                                               |
| 두 보스 동시 진행  | PA/70 #12117 Omega Multi-Missile과 #12124 Ultima Citadel Siege는 89ms 차이. `Omega:`/`Ultima:` 표시를 같은 phase에 함께 두고 일방의 sync로 다른 보스 시계를 이동시키지 않음                                                                                           |
| 공동 일정 종료     | Hnx/47 #22737에서 Omega HP=0, 그 뒤 #23468의 Ultima Mana Screen은 기존 공동 일정과 다른 시각. 공동 구간은 첫 보스 처치까지로 한정한다. KDL은 WasDefeated target으로 공동 예약을 취소하고 아래 단독 구간으로 이어진다                                                  |
| Kam 동시·대안      | PA/72 #19944 Illumed Facet·#19945 Empyreal Banish III는 동시 발동이므로 별도 표시. Elemental Blade와 Great Wheel ID 대안은 공통 이름/진행으로 요약                                                                                                                    |
| Kam 이른 진행      | `Mf29DAWxyrg1Tnpv/8` #11417 Scrivening 시작 → #12008 발동. 직전 간격이 약 3.7초 짧아 기존 Ability window를 벗어남. 6초 시전의 시작으로 보정해 5초 전 조회 이전에 복구. 단순 window 확대만으로 첫 발동 예고를 통과시키지 않음                                          |
| Kam 후반           | `aPVdHQmZJ9AbgwM6/24` #33705 두 번째 Palisade 뒤 #34266 Crystalline Resonance도 평가 범위에 들어오므로 표시 추가                                                                                                                                                      |
| Eald 진입 재사용   | PA/73 #609과 #22152는 같은 AD51 Uranos Cascade. 한 phase를 유지해 후반 재사용을 초기 진입으로 오인하지 않음. Phase Shift #14068/#14405는 표시와 sync                                                                                                                  |
| 마지막 Duplicate   | MFC/211 #32353은 마지막 Ancient Triad 후 약 8.6초. 원점 기준 350.9초로 작성. 전멸한 PA/73에서는 그전에 기록이 끝나 가짜 표본을 추가하지 않음                                                                                                                          |
| 동반 발동 요약     | Omega Blaster·미사일 연타, Ultima Crash·Siege 후속 타격, Sublime Elements의 속성들, Cronos/Vortex/Gaea/Javelin/Stellar의 후속 패킷과 Ancient Triad 동반 원소는 대표 기믹으로 요약. Energizing Equilibrium의 가변 시각·helper 개별 공격·속성/위치 순서는 추정하지 않음 |

시각은 대표 개발 경로의 간격을 0.1초로 근사해 다른 개발 자료와 대조했다. 반복되는 이름만으로 무한 loop를 만들지 않았다. Eald의 Phase Shift/Visions/Uranos, Kam의 Swordplay/Scrivening/Palisade/후반 Proving Ground를 제한된 window의 복구 신호로 사용한다.

## 범위와 투영

| 전투           | 대표 첫 Ability 이후 평가 상한       | 작성된 마지막 주요 기술                     |
| -------------- | ------------------------------------ | ------------------------------------------- |
| Kirin          | 230초                                | 두 번째 Wrought Arms 뒤 Bury                |
| Omega / Ultima | 238초 또는 첫 본체 HP=0 중 이른 시각 | 둘 다 살아 있는 공동 일정, 두 번째 Fire까지 |
| Kam'lanaut     | 325초                                | 두 번째 Palisade 뒤 Crystalline Resonance   |
| Eald'narche    | 357초                                | 두 번째 Ancient Triad 뒤 Duplicate          |

모든 구간은 FFLogs fight end에서도 끝난다. 완료·전멸 모두 기록된 범위까지만 평가하고 긴 입력의 미관측 후반을 통과로 계산하지 않는다. 첫 보류에서 공동 구간 이후의 Mana Screen·새 Shijin·이른 Kam 진행이 실패한 사실을 보존한다. 새 후보의 공동 구간 제한과 추가 대상은 **새 보류의 이벤트 확인 전에** 고정한다.

예상 이름별 main Boss actor 하나를 요구하고, Omega·Ultima는 같은 시도 안의 두 actor로 다룬다. HP가 75% 미만에서 90% 초과로 회복하면 재도전/큰 회복 가능성으로 제외한다. 범위 안 모든 cast·begincast를 Ability·StartsUsing으로 투영하고 이름 없는 시전자는 제외 근거다. 명시적 주요 ID 목록과 100ms 같은 이름 패킷 요약은 현재 `sandoria.evaluation.json`에 있다. 초기 평가의 TypeScript 투영은 당시 커밋에 보존한다. 모든 공격이나 선택 목록 밖의 새 기믹까지 평가했다는 뜻은 아니다.

공통 평가 프로필의 공동 구간은 FFLogs HP=0까지다. 아래 단독 구간은 처치 신호를 주입한 별도 재생으로 평가했다.

## Omega 처치 뒤 Ultima 단독 예고

[확장 개발 계획](../tests/fixtures/authored/sandoria-expanded-development-plan.json)의 공동 전투 일곱 건에서 원본 해시와 첫 처치 순서를 확인했다. 모두 Omega가 먼저 쓰러졌다. 관측된 Ultima의 새 주요 시전은 `AD09` Mana Screen과 `AD08` Energy Orb다. 첫 순서가 달라지고 일부 시전은 처치 전에 시작하므로 처치 시각만으로 첫 기술을 정하지 않는다.

| 개발 입력              | Omega HP=0 | 처치 뒤 시작해 완료된 시전의 원본 인덱스                                              |
| ---------------------- | ---------- | ------------------------------------------------------------------------------------- |
| `MFCVjw7RNch6mxtZ/208` | #24090     | Mana Screen #24503 → #24726                                                           |
| `vxCKdkn9y4D8FRBQ/57`  | #24922     | Mana Screen #25432 → #25741                                                           |
| `PAq8bNtxvXWG16QZ/70`  | #24483     | Mana Screen #24972 시작 후 완료 미관측                                                |
| `HnxFmz3CVZkNW1Xp/47`  | #22737     | Mana Screen #23263 → #23468, Energy Orb #23650 → #23856                               |
| `aPVdHQmZJ9AbgwM6/22`  | #23364     | Mana Screen #24125 → #24388. 진행 중이던 Energy Orb #23105 → #23394는 제외            |
| `Y3v4JFghpTjbnWwL/8`   | #22646     | 진행 중이던 Energy Orb #22609 → #22793만 완료. Mana Screen #23530 시작 후 완료 미관측 |
| `Zdxv3yCMtJq6QVcP/33`  | #22439     | Energy Orb #22769 → #23027                                                            |

Omega의 `WasDefeated`는 공동 예고를 닫고 대기 phase를 연다. 이후 Ultima의 새 `StartsUsing`에 맞춰 3.0초 뒤 발동을 표시한다. 기술별 phase 후보는 관측된 교대 경로를 따랐다. 같은 기술의 연속 시전은 일곱 입력에서 관측하지 못했다. Ultima 처치는 남은 예약을 취소한다.

개발 재생에는 원본 시작·완료 시각을 사용하고, 두 보스의 `WasDefeated`만 해당 HP=0 시각에 합성했다. 처치 뒤 시작해 완료된 여섯 시전은 실제 시전 시작부터 완료까지 2.974–2.987초였다. 시작 0.5초 뒤 조회에서 여섯 예고가 모두 대응했고 최대 오차는 26ms였다. 처치 전에 시작한 Energy Orb 두 건은 새 예고 대상에서 제외했고, 완료 전 처치된 시전은 성공으로 세지 않았다. `cargo test --test authored_sandoria`에서 합성한 두 초기 순서(서로 다른 pull의 시각을 조합), 공동 예약 취소, 진행 중 효과의 재진입 방지, 중복 Omega 처치와 Ultima 처치 뒤 취소를 확인한다.

Omega-first만 포함한 후보는 `bbb971c`다. 기존 공동 프로필·동결 manifest는 해당 후보의 기록으로 보존한다. 실제 ACT 처치와 시전의 순서 및 수정 후보의 독립 평가는 검증 과제로 남는다.

## Ultima 처치 뒤 Omega 단독 예고

기존 개발 계획의 공동 전투 17건은 모두 Omega-first였다. zone 1304, encounter 2068, 난이도 100의 보고서 탐색 16–17페이지에서 아홉 건을 추가 개발 자료로 수집했다. 여덟 건은 Omega-first였고, `6qp8bAmkahFDB3cx/6`은 Ultima-first였다. 이 입력 SHA-256은 `863386fbf749d7b3ad68a966cd1c5b8836f6c4b88551b0c223200c10ab814c5a`다.

| 원본 인덱스 | 관측                                    | 첫 처치 기준 |
| ----------- | --------------------------------------- | ------------ |
| #21428      | Ultima HP=0                             | 0.000초      |
| #21834      | Omega `AD23` Trajectory Projection 시작 | +5.159초     |
| #22137      | Omega `AD23` 발동                       | +8.631초     |

Ultima 처치가 공동 예약을 닫고 Omega 대기 phase를 연다. 새 `AD23` 시전 시작 뒤 3.5초에 효과를 표시한다. 같은 처치 패킷이 다시 오더라도 생존자의 예약을 유지하도록 각 완료 신호의 활성 phase를 제한한다. Omega 처치 때 남은 예약을 닫는다.

추가 개발 재생은 위 Omega 효과 하나와 Ultima의 완료 효과 다섯 개를 시전 시작 0.5초 뒤에 모두 예고했다. Omega의 오차는 28ms, Ultima의 최대 오차는 17ms였다. 첫 처치 신호는 원본 HP=0 위치에 합성했으며 이 Omega 기록에는 Omega 자신의 HP=0 패킷이 없었다. Omega의 예약 취소는 별도 합성 회귀로 확인했다. 같은 보스의 미관측 후속 시전이나 두 방향의 실제 ACT 전달은 검증 범위에 남는다.

기술별 phase를 쓰던 후보 `37abbf4`의 KDL SHA-256은 `b559358ba0489f9665996c799be69213c26145b9d1ba9afc23e67d7f6f0c2bf8`이다.

## sequence로 단독 일정 재작성

생존자별 대기 phase와 첫 처치 조건을 유지하고, 기술별 phase 세 개를 각 생존자 아래의 sequence로 옮겼다. 새 시전에서 Mana Screen·Energy Orb는 3.0초, Trajectory Projection은 3.5초 뒤 표시한다. 시전마다 새 sequence 실행이 생겨 같은 Mana Screen이 연속으로 시작된 합성 사례에서도 각각 예고했다. 생존자 처치로 해당 phase를 닫으면 그 하위 예약도 취소된다.

앞의 개발 기록 16개를 기술별 phase 후보 `37abbf4`와 새 후보로 각각 재생했다. 원본 시전 시각과 HP=0 위치에 합성한 처치 신호는 양쪽에 동일하게 공급했다. 실행된 표시 ID와 시각, 시전 시작 0.5초 뒤의 미래 조회 21건이 양쪽에서 일치했다. 현재 KDL SHA-256은 `ef8d2afb0f1ef6eec1e7da5880a56aeb9a3629cbe94d16cbf352b48a5be54446`이다. 공통 평가 프로필과 기존 동결 자료는 공동 구간 평가에 계속 사용한다. 수정 후보의 독립 평가와 실제 ACT 처치 시각은 검증 과제다.

## 고정 수정 후보의 평가

| 전투           | 확장 개발: 적격·표시·조회·최대 조회 오차 | 새 보류: 적격·표시·조회·최대 조회 오차 |
| -------------- | ---------------------------------------- | -------------------------------------- |
| Kirin          | 6개 · 106/106 · 100/100 · 972ms          | 4개 · 77/77 · 73/73 · 1,111ms          |
| Omega / Ultima | 7개 · 223/223 · 209/209 · 357ms          | 4개 · 132/132 · 124/124 · 373ms        |
| Kam'lanaut     | 7개 · 242/242 · 235/235 · 300ms          | 4개 · 141/141 · 137/137 · 232ms        |
| Eald'narche    | 6개 · 166/166 · 160/160 · 310ms          | 4개 · 111/111 · 107/107 · 410ms        |

확장 개발은 28개 중 중복 2개를 제외한 적격 26개 모두 통과했다. 새 보류는 제외 없이 16개 모두 통과했고 표시 461건·사전 조회 441건이 대응했다. 이 중 437건은 5초 전, 첫 Shijin 선택 발동 4건은 2초 전 조회다. 안전한 보스 진입과 실행 진단도 확인했다. 허용 오차·선택 목록·범위를 평가 후 완화하지 않았다.

| Shijin 관측 선택 | 개발 | 새 보류 |
| ---------------- | ---- | ------- |
| Quake            | 1    | 3       |
| Stonega III      | 2    | 1       |
| Genbu            | 2    | 0       |
| Eastwind         | 1    | 0       |

새 보류는 Quake·Stonega 선택을 독립적으로 지지한다. Genbu·Eastwind는 개발과 합성 검사만 있으므로 네 선택 모두 독립 검증했다고 주장하지 않는다. 유한 경로 이후의 반복 역시 검증하지 않았다. 44개 입력 파일·각 계획·후보의 해시와 개발/새 보류 간 보고서 중복 없음도 확인했다. 모든 변형 업로드의 독립성까지 증명한 것은 아니다.

## 검증과 재현

| 검사        | 내용                                                                                                                                                                                                                      |
| ----------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 구조·합성   | 정적 구조, Rust 17개: 네 보스 종료/재시작, 네 Shijin의 공통 기술 사전 예고와 합류, 선택 누락 unresolved·후속 복구, Uranos 재사용, 신호 누락 뒤 +7초 복구, 두 보스 동시 표시·이름을 지정한 처치 취소, 이른 Scrivening 보정 |
| 투영        | 동시 대상·패킷 요약·StartsUsing·유한 범위·전멸·첫 처치 경계·이름 누락/HP 회복 제외·원본 불변성                                                                                                                            |
| 공통 평가기 | 대상별 lead만 확장. Clyteum·Mistwake 기존 보류의 모든 표본·제외·집계가 변경 전과 같음. 평가 중 KDL이 바뀌면 보고서를 내지 않는 보호와 회귀 검사 추가                                                                      |

최초 평가와 수정 후보의 결과·후보 커밋은 `sandoria.validation.json`에서 구분한다. 첫 보류 16개를 수정 후보의 독립 평가로 재사용하지 않는다.

```sh
cargo build
target/debug/btimeline validate timelines/sandoria.kdl
bun run --cwd scripts test
cargo test --test authored_sandoria
```

보고서는 새 경로에만 쓴다. 초기 후보는 `784924f` 체크아웃에서 초기 계획으로 재현한다. `sandoria-recovery-development-plan.json`은 단독 예고 추가 전인 `61677e1`의 KDL과 프로필로 실행한다. 수정에 보류 결과를 사용하면 해당 입력을 개발 자료로 전환한다. 실제 ACT 신호/시전자 이름·종료 동등성, 실제 UI, 단독 생존 보스 후반·표의 경로 이후·helper 개별 공격·대상/위치는 미검증이다.

## 이벤트 ID 평가가 드러낸 표시 24건 검토

경고 검토 당시 KDL SHA-256은 `589afddf3467a6865e7057ee73fe88a0075283a6671839e35a5a83d0b965640b`다. 원래 개발·보류를 합친 44개 입력을 [경고 검토 계획](../tests/fixtures/authored/sandoria-warning-review-plan.json)에 개발 역할로 고정했다. 42개 적격 입력의 1차 재평가가 통과했고 원래 이름 누락/본체 불명확 제외 2개를 유지했다. 과거 manifest·validation은 덮어쓰지 않았다. 이 재평가는 독립 검증 표본으로 계산하지 않는다.

| 원인                                | 건수 | 근거와 처리                                                                                                                                   |
| ----------------------------------- | ---- | --------------------------------------------------------------------------------------------------------------------------------------------- |
| Cronos Sling alias 누락             | 5    | Hnx/50 #1037의 AD4C는 첫 Sling. AD49/AD4A/AD4B와 같은 표시 후보·패킷 그룹에 추가                                                              |
| Striking Left/Right alias·표시 누락 | 9    | Zdx/31 #17205 ADAF, Hnx/45 #16674 ADAF·#17271 ADA4. 기존 3개 Wringer/Strike 슬롯의 이름과 평가 ID를 확장. 시각·선택·sync는 유지               |
| Smiting Sequence alias·표시 누락    | 3    | Zdx/31 #17745 AD9F는 같은 중간 슬롯의 Smiting Left Sequence. AD9E/AD9F를 추가하고 표시 대안을 명시                                            |
| Bury 보조 시전자                    | 4    | Hnx/45 #21963 등은 Chiseled Arm의 ADBF. 보조 시전자 발동으로 평가하고 동시 여러 팔의 패킷은 한 표시로 요약. 본체 유일성 검사는 Kirin에만 적용 |
| 끝부분 발동 미관측                  | 3    | 아래 표의 시전 시작은 있으나 발동은 기록되지 않음. 불필요한 표시로 단정하거나 표본을 제외하지 않고 검토 경고 유지                             |

| 입력·표시       | 원본 근거                                 | 해석 한계                                                   |
| --------------- | ----------------------------------------- | ----------------------------------------------------------- |
| PA/72 `kam-36`  | #32287 ACB7 시전 시작, 전투 끝 4.585초 전 | 발동 미관측. 정상 발동 완료·실제 종료 신호 동등성은 미확인  |
| PA/73 `eald-29` | #31144 AD77 시전 시작, 끝 3.708초 전      | 발동 미관측. 이 로그로 마지막 Duplicate를 삭제하지 않음     |
| Y3/6 `kirin-15` | #18660 ADB1 시전 시작, 끝 8.774초 전      | 발동 미관측. 기록 종료 이전 취소·진행 중단 여부는 확인 불가 |

`out/sandoria-warning-review-v2.json`에서 대응 없는 표시가 24건에서 위 3건으로 줄었다. 시간표·window 완화나 입력 제외 확대 없이 누락된 대안/시전자를 보완했다. 보조 actor를 공동 본체로 취급하는 중간 profile은 거부하고 바로 수정했으며 해당 중간 결과를 검증 근거로 사용하지 않는다.

## 수정본의 새 독립 평가

[사전 고정 기록](../tests/fixtures/authored/sandoria-body-fresh-freeze.json)은 전투 이벤트를 읽기 전에 KDL·profile·평가기 실행 파일·판정 기준을 고정했다. zone 1304의 보고서 탐색 5쪽에서 기존 개발·보류에 없던 6개 보고서를 골랐다. [다운로드 계획](../tests/fixtures/authored/sandoria-body-fresh-download-plan.json)의 24개를 모두 [보류 평가](../tests/fixtures/authored/sandoria-body-fresh-holdout-plan.json)에 포함하고 기존 개발의 기술열 fingerprint를 제외 목록으로 넣었다. [요약 기록](sandoria.fresh-evaluation.json)에 원본·보고서 해시, 판정, 선택 원본 인덱스 및 실패를 보존한다. 후보와 기준은 평가 후에도 바꾸지 않았다.

첫 묶음은 제외 0개, 24개 중 23개가 통과했다. Kirin은 Quake 1회, Stonega 4회, Eastwind 1회가 모두 통과했고 Genbu는 관측되지 않았다. Eastwind `ZzqnvApcPVYNDmH2/17`의 AD82 선택은 #9818, Riddle은 #10397, Double Cast는 #11252, 동시 Quake·Stonega는 #12107/#12118이다. 해당 선택 표시의 사전 조회 오차는 최대 92ms였다.

실패 `LbWnM34CRBc6xHFd/8`은 Eald의 첫 Cronos Sling을 #894(전투 기준 21.007초)에 기록했지만, 진입 신호로 쓰는 AD51의 첫 발동은 #22377(265.426초)이다. 초기 AD51이 관측되지 않아 후반 Uranos를 첫 진입으로 받아들였고, 그 뒤 Duplicate #23026 등 6개의 후속 표시·예고를 놓쳤다. 원본이 무결성 검사를 통과했으므로 실패로 보존한다. 실제 초반 기술 생략과 관측 누락은 이 자료로 구분할 수 없다. 복구 신호를 바꾸는 후속 실험에서는 이 보고서를 개발로 전환한다.

Genbu 표본을 얻기 위해 같은 후보와 기준으로 Kirin만 추가했다. zone 탐색 6–7쪽의 9개 후보 중 전투 길이·교전 시간·결과가 모두 같은 업로드 후보 하나를 이벤트 다운로드 전에 보수적으로 제외했다. [추가 고정 기록](../tests/fixtures/authored/sandoria-kirin-followup-freeze.json)과 [평가 계획](../tests/fixtures/authored/sandoria-kirin-followup-holdout-plan.json)의 새 보고서 8개 중 적격 7개에서 5개가 통과했다. `bBH61QKjfNnDVyzg/18`은 #13683의 시전자 ID 310을 원본 actor 이름으로 투영할 수 없어 제외했다.

| 선택·결과                              | 독립 관측 근거                                                                                                                                             |
| -------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Genbu 1회 통과                         | `vnwPcLFD9A8k4mfJ/27` #8322 선택                                                                                                                           |
| Eastwind 2회 통과                      | 첫 묶음 `ZzqnvApcPVYNDmH2/17` #9818 및 추가 묶음 `cptvaKBRfn9YMrQ2/3` #9401                                                                                |
| Eastwind 후속과 일치하나 선택 실패 2건 | `NGytdwK3ZWTjn78C/16` #10639, `z1BXyNRgnxkLac2h/2` #9263에서 Riddle 시전 시작. 두 로그에 AD82 선택이 없고 Riddle → Double Cast → 동시 Quake·Stonega를 관측 |

선택 실패 2건은 `unresolved`와 예고 실패로 보존한다. Riddle을 무조건 선택 신호로 쓰면 Genbu와 충돌할 수 있고, 관측된 약 4.99초 시전으로는 기존 5초 목표를 복구할 수 없다. 후보·판정 기준을 이 보류 결과에 맞춰 바꾸지 않았다. 두 묶음 합계는 32개 선정·1개 제외·31개 적격·28개 통과다. 최초 신호가 없는 경우의 복구는 독립 검증의 다음 과제다.

```sh
out/btimeline-body-frozen evaluate tests/fixtures/authored/sandoria-body-fresh-holdout-plan.json --profile timelines/sandoria.evaluation.json -o out/sandoria-body-fresh-holdout-new.json
out/btimeline-body-frozen review out/sandoria-body-fresh-holdout-new.json
out/btimeline-body-frozen evaluate tests/fixtures/authored/sandoria-kirin-followup-holdout-plan.json --profile timelines/sandoria.evaluation.json -o out/sandoria-kirin-followup-new.json
```

실행 파일은 로컬 `out/`에 보존했다. 재빌드한 파일의 해시가 다르면 원래 동결 평가로 실행할 수 없으며 새 평가 계획으로 구분한다. 오프라인 원본 투영은 ACT·외부 실행기·실제 UI를 검증하지 않는다.

## Eastwind 선택 신호 누락 복구

AD82 없이 진행한 두 로그에서 Double Cast 시전 시작 A916을 확인했다. 원본 위치는 `NGytdwK3ZWTjn78C/16` #11658과 `z1BXyNRgnxkLac2h/2` #10176이다. Shijin 선택이 비어 있으면 이 신호부터 복구 일정을 시작한다. 이전에 AD82로 선택한 일정은 계속 진행한다.

두 로그에서 선택 미해결 진단이 사라졌다. 동시 Quake·Stonega의 5초 전 조회 오차는 각각 68ms와 32ms였고, Wrought Arms는 31ms와 45ms였다. Riddle 표시와 Double Cast 자체의 5초 전 예고는 여전히 실패한다. 두 로그 모두 전체 판정은 실패로 남는다.

Eastwind 복구만 적용한 `59b41c3`에서는 76개 중 3개를 제외하고 적격 73개를 평가했다. 70개가 통과했고, 두 Eastwind 로그와 초기 진입 신호가 누락된 Eald 로그가 실패했다. 위 독립 평가 기록은 당시 후보의 결과다.

## Eald 초기 진입 신호 누락 복구

`LbWnM34CRBc6xHFd/8`에서는 첫 Uranos Cascade가 관측되지 않았고 첫 Cronos Sling이 #894에 남아 있다. 활성 phase가 없을 때 이 Sling을 받으면 `resume-at=13.1`로 Eald 일정에 진입한다. 복구 시점에 Sling을 표시하고, 그보다 앞선 Uranos Cascade를 건너뛴다. 이미 활성화된 Eald 일정은 이후 Sling에서도 계속 진행한다.

이 로그의 적격 대상 26개가 통과했다. 진입 뒤 5초 전 조회를 수행한 대상의 최대 오차는 269ms였다. Sling 자체는 진입과 같은 시각에 표시된다. 이 복구는 관측된 첫 Sling이 일정의 첫 Sling이라는 가정에 의존한다. 앞쪽 Sling까지 누락된 자료와 수정 후보의 독립 보류 자료는 추가 검증이 필요하다.

전체 개발 회귀에서는 76개 선정 중 3개가 제외되고 적격 73개 중 71개가 통과했다. Eald 한 건이 실패에서 통과로 바뀌었다. 나머지 두 실패는 Eastwind의 초기 표시와 예고에 해당한다.

## Eastwind의 초기 예고 복구

`NGytdwK3ZWTjn78C/16` #10476과 `z1BXyNRgnxkLac2h/2` #9148에서 Kirin의 ADCD 발동을 확인했다. 발동 시각은 phase 시작 후 각각 91.645초와 90.984초였으며 약 6.7초 뒤 Riddle이 발동했다. 비교한 Genbu 자료 3개에서는 같은 ADCD가 104.108–104.745초에 나타났다.

선택이 비어 있고 ADCD가 89.3–93.3초에 들어오면 이른 Eastwind 복구 일정에 진입한다. AD82의 정상 선택은 86–94초에 받으며, 더 늦게 반복되는 AD82는 선택 신호에서 제외한다. 두 이른 신호가 모두 빠진 경우 Double Cast 시전 시작에서 이어가는 복구 경로를 사용한다.

두 실패 로그 모두 기존 5초 예고 기준을 통과했다. Riddle 예고 오차는 30ms와 36ms, Double Cast는 70ms와 2ms였다. ADCD에 의미를 부여하는 근거는 이 자료의 시각 차이이며, 새 후보와 다른 진행 속도에서의 구분은 독립 검증이 필요하다.

전체 개발 회귀에서 76개 선정 중 3개를 제외했고 적격 73개가 모두 통과했다.

## ADCD 복구 후보의 새 독립 검증

`ea9cb83`의 KDL·profile·평가기·판정 기준과 입력 선정을 이벤트 다운로드 전에 고정했다. Kirin 탐색 8–11쪽과 산도리아 구역 탐색 12–13쪽에서 이전에 사용한 보고서와 메타데이터상 중복 가능성이 있는 입력을 걸러, 새 보고서 29개의 전투 43개를 선정했다. 원본 평가에는 이전 개발의 기술열 fingerprint 제외 목록도 적용했다. [고정 기록](../tests/fixtures/authored/sandoria-adcd-independent-freeze.json) · [평가 계획](../tests/fixtures/authored/sandoria-adcd-independent-holdout-plan.json)

| 전투             | 선정 | 제외 | 적격 통과 | 실패 |
| ---------------- | ---: | ---: | --------: | ---: |
| Faithbound Kirin |   28 |    1 |        27 |    0 |
| Omega·Ultima     |    6 |    0 |         0 |    6 |
| Kam'lanaut       |    5 |    0 |         5 |    0 |
| Eald'narche      |    4 |    0 |         4 |    0 |
| 합계             |   43 |    1 |        36 |    6 |

Kirin 제외 1건은 시전자 이름을 해석할 수 없는 원본 패킷이다. Omega·Ultima의 6건은 첫 실패가 Ultima의 Tractor Field였으며, 예정 표시가 관측보다 17.484–18.685초 늦었다. 이 결과를 실패로 보존했다. 원본별 첫 실패와 제외 근거는 [검증 결과](sandoria.adcd-independent-validation.json)에 있다.

ADCD와 독립적으로 Kirin의 Double Cast로 Eastwind, Moonbound Genbu의 AD84 시전 시작으로 Genbu를 분류했다. 아래 ADCD 시각은 Kirin phase 진입 기준이고 전투 길이는 기록 전체 기준이다.

| 경로     |            표본 | 전투 길이         | 선택 범위 안 ADCD | 원본 결과                | 초기 선택 신호 제거 재생                           |
| -------- | --------------: | ----------------- | ----------------- | ------------------------ | -------------------------------------------------- |
| Eastwind | 7개, 보고서 6개 | 157.990–269.833초 | 91.186–91.925초   | 7개 통과, ADCD 복구 4개  | AD82 제거 후 7개 모두 ADCD로 복구 및 5초 예고 통과 |
| Genbu    | 9개, 보고서 9개 | 160.903–266.579초 | 104.039–105.100초 | 9개 모두 Genbu 선택·통과 | AD84 제거 후 9개 모두 미해결, Eastwind 오선택 0개  |

Eastwind의 신호 제거 재생에서 Riddle과 Double Cast의 5초 전 예고 오차는 1–63ms였다. 사본에서 제거한 원본 인덱스와 변환 후 입력 해시는 검증 결과에 보존했다. 이 사본은 합성 검사로 구분한다.

이번 표본은 전투 길이가 넓게 달라도 선택 시각의 분리가 유지됐다. Genbu의 가장 이른 ADCD는 Eastwind 창의 상한 93.3초보다 약 10.7초 늦었다. 현재 구분은 시각에 의존하므로, AD84가 없고 Genbu ADCD가 89.3–93.3초 안에 들어오는 진행에서는 Eastwind로 선택될 수 있다. 해당 시각대의 Genbu와 ACT·실제 UI는 검증 범위에 남아 있다.

```sh
out/btimeline-eastwind-independent-frozen evaluate tests/fixtures/authored/sandoria-adcd-independent-holdout-plan.json --profile timelines/sandoria.evaluation.json -o out/sandoria-adcd-independent-holdout-new.json
out/btimeline-eastwind-independent-frozen review out/sandoria-adcd-independent-holdout-new.json
```

### Kirin의 빠른 처치와 느린 처치 재검증

`417437f`의 KDL·profile·평가기 해시를 이벤트 다운로드 전에 고정했다. 산도리아 구역 탐색 14–15쪽에서 기존 로컬 보고서와 겹치는 코드를 빼고, 보고서당 1개씩 빠른 처치 6개와 느린 처치 6개를 선정했다. 기술열 지문이 중복된 1개를 제외한 원본 11개는 phase 진입과 예고를 모두 통과했다. [다운로드 계획](../tests/fixtures/authored/sandoria-new-independent-download-plan.json) · [평가 계획](../tests/fixtures/authored/sandoria-new-independent-evaluation-plan.json)

| 원본에서 독립 신호로 분류한 경로 | 표본 | 전체 전투 길이    | Kirin 진입 기준 ADCD | 초기 선택 신호 제거 사본의 결과                                                          |
| -------------------------------- | ---: | ----------------- | -------------------- | ---------------------------------------------------------------------------------------- |
| Eastwind                         |    4 | 180.771–294.368초 | 90.872–91.637초      | AD82 제거 후 4개 모두 ADCD로 선택. 전환 후 첫 Crimson Riddle의 5초 전 예고 오차는 9–16ms |
| Genbu                            |    1 | 261.506초         | 104.639초            | Moonbound Genbu의 AD84 시전 시작 제거 후 미해결 1개. Eastwind 오선택 0개                 |

나머지 원본 6개는 Quake 또는 Stonega를 선택했다. 제거 사본은 합성 검사다. 이전 검증과 같이 Faithbound Kirin의 Double Cast 완료와 Moonbound Genbu의 AD84 시전 시작으로 원본 경로를 분류한 뒤, 각 경로의 초기 선택 패킷만 제거하고 `collection.eventCount`를 갱신해 재생했다. 새 Genbu 표본은 느린 처치 한 건이며, 89.3–93.3초 창에 들어오는 Genbu ADCD는 관측되지 않았다.
