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
| 공동 일정 종료     | Hnx/47 #22737에서 Omega HP=0, 그 뒤 #23468의 Ultima Mana Screen은 기존 공동 일정과 다른 시각. 공동 구간은 첫 보스 처치까지로 한정하고 생존 보스의 단독 후반은 작성하지 않음. 실제 KDL은 WasDefeated target으로 공동 예약을 취소                                       |
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

FFLogs HP=0은 구간 경계만 정하며 WasDefeated를 합성하지 않는다. WasDefeated의 취소는 별도 합성 입력으로 검사하므로 실제 ACT 검증으로 세지 않는다. 첫 처치 뒤 단독 생존 보스의 예고는 범위 밖이다. KDL은 작성한 유한 경로 뒤 새 일정을 만들지 않는다.

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
target/debug/btimeline evaluate --profile timelines/sandoria.evaluation.json tests/fixtures/authored/sandoria-recovery-development-plan.json -o out/sandoria-recovery-development-new.json
bun run --cwd scripts test
cargo test --test authored_sandoria
```

보고서는 새 경로에만 쓴다. 초기 후보는 `784924f` 체크아웃에서 초기 계획으로 재현한다. 수정에 보류 결과를 사용하면 해당 입력을 개발 자료로 전환한다. 실제 ACT 신호/시전자 이름·종료 동등성, 실제 UI, 단독 생존 보스 후반·표의 경로 이후·helper 개별 공격·대상/위치는 미검증이다.

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

복구에 사용한 로그를 포함한 [개발 회귀 계획](../tests/fixtures/authored/sandoria-recovery-development-plan.json)에서 76개 중 3개를 제외하고 적격 73개를 평가했다. 70개가 통과했고, 두 Eastwind 로그와 초기 진입 신호가 누락된 Eald 로그가 실패했다. 위 독립 평가 기록은 당시 후보의 결과다. 새 후보의 독립 보류 평가와 Eald의 초기 진입 신호 누락 복구가 남아 있다.
