# 산도리아: 두 번째 반향세계

2026-11-03. `timeline-author` skill로 원본 FFLogs의 cast·begincast·actor 관측을 해석했다. cactbot 전투 시간표·공략·영상은 사용하지 않았다. 게임 zone 1304와 보스 이름은 수집 메타데이터다. 최종 KDL은 FFLogs나 작성 기록 없이 실행한다.

| 항목         | 범위                                                                                                                                                                        |
| ------------ | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 산출물       | [sandoria.kdl](sandoria.kdl), 네 전투·121개 표시·Shijin 분기/합류                                                                                                           |
| 후보 SHA-256 | `71515ca84e56a05841ac4930c522369448e889807237510d43e316cd48345d6b`                                                                                                          |
| 개발 입력    | [개발 계획](../tests/fixtures/authored/sandoria-development-plan.json)의 12개 파일·해시. PA 보고서의 초기 네 파일도 개발 자료이며 원본 복사본을 독립 입력으로 추가하지 않음 |
| 개발 평가    | 동일 상대 시각·ID의 중복 업로드 2개 제외, 적격 10개 통과. 초기 후보의 마지막 Duplicate 오차는 개발 원본에서 수정                                                            |
| 평가 기준    | 주요 기술 첫 관측 5초 전 조회, 오차 1.5초 이하. Shijin 첫 Quake/Stonega III만 2초 전: 선택 cast 시작부터 실제 발동까지 약 2.98초이므로 5초 전에는 경로가 아직 미정          |
| 진입         | 첫 대표 Ability에서 해당 보스를 시작. 진입 기술 자체와 진입 후 5초 미만 기술의 5초 전 예고는 평가하지 않음                                                                  |
| 독립 평가    | 후보·입력 역할·기준을 고정한 뒤 미사용 보고서의 이벤트를 평가. 결과는 후속 절에 기록                                                                                        |

## 해석과 원본 근거

`#`는 해당 파일의 0부터 시작하는 `raw.events` 인덱스다. [검증 기록](sandoria.validation.json)에 모든 평가 대상 인덱스·시각·ID를 보존한다.

| 결정              | 원본 근거·표시/제어 영향                                                                                                                                                                                |
| ----------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Kirin 도입        | `PAq8bNtxvXWG16QZ/68` #1328 Stonega IV → #2727 Wrought Arms → #3813/#4716 Synchronized Strike → #5962/#6494 Crimson Riddle → #7696 Summon Shijin. 두 Crimson Riddle은 7초 떨어진 별개 표시              |
| Shijin 선택       | PA/68 #10079 begincast B079 → #10262/#10899 Stonega III. `MFCVjw7RNch6mxtZ/206` #10233 begincast B07B → #10505 및 후속 B07C Quake. 이름·간격과 후속 진행이 달라 두 option으로 작성                      |
| 공통 합류         | PA/68 #12105, MFC/206 #12685의 두 번째 Wrought Arms. 첫 Wrought Arms와 겹치지 않는 window로 합류하고 후속 간격을 여기서 시작. 합류 기술 자체는 공통 예정 시각의 표시로 먼저 예고                        |
| Kirin 대안        | Striking Left/Right, Wringer/Synchronized Strike 순서가 바뀌지만 공통 후속 Mighty Grip·Deadly Hold·Bury가 이어짐. 대안 표시를 `/`로 묶으며 위치·안전 지점을 추정하지 않음                               |
| 두 보스 동시 진행 | PA/70 #12117 Omega Multi-Missile과 #12124 Ultima Citadel Siege는 89ms 차이. 서로 대안이 아니므로 `Omega:`/`Ultima:`로 표시를 분리하고 같은 phase에 함께 둠. 일방의 sync로 다른 보스까지 이동시키지 않음 |
| 동반 발동 요약    | Omega Blaster·미사일 후속 패킷, Ultima Crash·Citadel Siege 후속 타격을 대표 첫 기술로 요약. Energizing Equilibrium은 개발 로그마다 시각이 달라 고정 예고에서 제외                                       |
| Kam'lanaut        | PA/72 #19908 Elemental Resonance 뒤 #19944 Illumed Facet·#19945 Empyreal Banish III가 동시에 발동하므로 별도 표시. Sublime Elements는 여러 속성 동반 발동을 요약하며 속성 순서를 만들지 않음            |
| Kam 대안·복구     | AC91/AC92 Elemental Blade와 ACAD–ACB0 Great Wheel은 공통 이름/진행으로 표현. Swordplay·Scrivening·Palisade·후반 Proving Ground를 제한된 window의 sync로 사용                                            |
| Eald'narche       | PA/73 #609과 #22152는 같은 AD51 Uranos Cascade. 한 phase를 유지해 후반 재사용을 초기 진입으로 오인하지 않음. Phase Shift #14068/#14405는 표시와 sync이며 새 phase 진입으로 만들지 않음                  |
| Eald 요약         | Cronos Sling 후속 AD4F/AD50, Gaea Stream 연타, Vortex/Javelin/Stellar Burst의 후속 타격은 대표 기술로 요약. Ancient Triad와 속성 동반 발동도 대표 표시만 사용하며 원소·위치 선택을 추정하지 않음        |
| 마지막 Duplicate  | MFC/211 #32353은 마지막 Ancient Triad 후 약 8.6초. 전체 오프셋 350.9초로 작성. 전멸한 PA/73에서는 그 전에 관측이 끝나므로 가짜 표본을 추가하지 않음                                                     |

초안 그래프를 실행 일정의 근거로 승격하지 않고 관측에서 직접 작성했다. 표시 시각은 대표 개발 경로의 간격을 0.1초로 근사하고 다른 개발 자료로 대조했다. 무한 loop를 뒷받침할 실제 재반복 경계가 없어 유한 경로로 작성했다.

## 관측 투영과 유한 범위

| 전투             | 평가 상한: 대표 첫 Ability 이후 | 작성된 마지막 주요 기술                |
| ---------------- | ------------------------------- | -------------------------------------- |
| Faithbound Kirin | 230초                           | 두 번째 Wrought Arms 뒤 Bury           |
| Omega / Ultima   | 238초                           | 두 번째 Fore-to-Aft / Aft-to-Fore Fire |
| Kam'lanaut       | 325초                           | 두 번째 Esoteric Palisade              |
| Eald'narche      | 357초                           | 두 번째 Ancient Triad 뒤 Duplicate     |

종료 시각은 위 상한과 FFLogs fight end 중 이른 값이다. 긴 보류 로그에서 미관측 후반을 통과로 계산하지 않도록 **평가 전에** 상한을 고정한다. 완료·전멸 모두 기록된 범위까지만 평가하며 HP=0을 합성 ACT 종료 신호로 만들지 않는다. 실제 KDL은 유한 표시가 끝나면 새 일정을 예측하지 않는다.

예상 보스 이름마다 main Boss actor 하나를 요구하며 Omega·Ultima는 둘을 같은 시도로 취급한다. HP가 75% 미만에서 90% 초과로 회복한 자료는 재도전/큰 회복의 불확실성으로 제외한다. 범위 안의 모든 cast·begincast를 각각 Ability·StartsUsing으로 투영하며 이름 없는 시전자는 제외 근거다. 주요 대상은 `sandoria_recording.ts`의 명시적 ID 목록이고 100ms 이내 같은 표시 이름의 대상별 패킷을 한 번으로 센다. 이 선택 목록 밖의 모든 공격을 재현했다는 뜻은 아니다.

## 실행·복구 검사

| 검사      | 결과                                                                                                                                                                                                           |
| --------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 정적 검사 | 독립 KDL 구조 통과                                                                                                                                                                                             |
| 합성 Rust | 9개 통과: 네 보스 종료/재시작, 두 Shijin 선택의 발동 전 조회와 공통 합류, 선택 누락을 unresolved로 남기면서 후속 복구, Uranos 재사용 안전성, Phase Shift 누락 후 다음 신호의 +7초 복구, 두 보스 동시 미래 표시 |
| 투영 검사 | 동시 대상 보존·패킷 요약·StartsUsing 매핑·유한 범위·전멸 허용·알 수 없는 시전자/HP 회복 제외·입력 불변성                                                                                                       |
| 공통 도구 | 기존 던전 평가기를 재사용. 대상별 `lead_ms`만 추가해 2초 표본을 별도로 기록하고 다른 대상은 기존 manifest의 5초 기준 유지                                                                                      |

```sh
cargo build
target/debug/btimeline validate timelines/sandoria.kdl
bun scripts/verify_authored_sandoria.ts target/debug/btimeline tests/fixtures/authored/sandoria-development-plan.json out/sandoria-authoring/development-new.json
bun scripts/verify_authored_sandoria.ts target/debug/btimeline tests/fixtures/authored/sandoria-holdout-plan.json out/sandoria-authoring/holdout-new.json
bun run --cwd scripts test
cargo test --test authored_sandoria
```

보고서는 새 경로에만 저장한다. 후보 수정에 보류 결과를 사용하면 그 입력을 개발 자료로 전환한다. 실제 ACT의 이벤트·시전자 이름·시작/종료 동등성, 실제 UI, helper 개별 공격·대상/위치, 표에 정한 경로 이후의 진행은 미검증이다.
