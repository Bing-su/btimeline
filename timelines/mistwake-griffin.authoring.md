# Mistwake Griffin 작성 기록

2026-11-02. [timeline-author skill](../.agents/skills/timeline-author/SKILL.md)의 첫 실제 로그 적용이다. 대상은 dungeon mode의 Thundergust Griffin 한 보스이며, 다른 두 보스와 던전 이동은 포함하지 않는다.

## 산출물·입력 역할

| 항목             | 값                                                                                                                                                                             |
| ---------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| 최종 KDL         | [mistwake-griffin.kdl](mistwake-griffin.kdl), 18개 이벤트 정의                                                                                                                 |
| KDL SHA-256      | `1e43a7efbec38729f14bf33e26755bfc4f1cf6d55d9815b53a5fe893c05c4ce2`                                                                                                             |
| 개발 코퍼스      | `logs/loop-mistwake/*.json` 8개 + `logs/mistwake-longer/*.json` 4개. 전부 개발 자료                                                                                            |
| 원본 근거        | [griffin-samples.json](../tests/fixtures/authored/griffin-samples.json)에 12개 파일 해시·report/fight·원본 시작/완료 인덱스·선정한 204건의 원본 이벤트 인덱스와 상대 시각 보존 |
| fixture SHA-256  | `b51452bb6a4ed8384275dc93fb858952f307c8ec140a05e66540e9a64adab104`                                                                                                             |
| 대표 경로        | `YQAawtmnD6FRVzPj/80`. 세 번째 Thunderspark까지 관측. 확장 자료 `nfx8gFT3b26kqyLM/11`도 같은 후반 경계를 지지                                                                  |
| 기존 자동 초안   | `out/griffin-authoring/baseline.{kdl,evidence.json}`, 기존 8개 입력으로 재생성. Griffin의 유한 17개 + 반복 32개 = 49개 정의                                                    |
| 초안 KDL SHA-256 | `2a0a9b7c3daa4b797766f0e8243b24673d0e33382712e67ae2554301c0138df8`. 생성 evidence는 수정하지 않음                                                                              |
| 독립 평가        | 미실시. 이 자료들은 이미 구현·평가에 사용됐으므로 미사용 보류 자료로 세지 않음                                                                                                 |

## 해석과 작성 결정

| 결정                             | 원본 근거·이유                                                                                                                      | 영향과 한계                                                                                                                               |
| -------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------- |
| 첫 Thunderspark를 0으로 사용     | 대표의 원본 `#15471`. 12개 모두 자동 공격 뒤 첫 주요 시전은 `B0EB`                                                                  | 첫 시전 자체의 사전 예고는 범위 밖. 같은 진입 신호의 재등장이 활성 phase를 재시작하지 않음                                                |
| 첫 회차와 후반 회차 분리         | 대표에서 두 번째 `B0EB #17151`까지 81.809초. 첫 회차에는 Storm Surge가 없고, 이후 `B0F3 #17430`이 등장                              | 유한 opener 8개 뒤 81.8초에 후반 loop 시작. 초기 단계를 후반 주기에 억지로 맞추지 않음                                                    |
| 반복 주기 89.3초                 | 두 번째→세 번째 Thunderspark 간격은 대표 89.392초, `nfx8gFT3b26kqyLM/11` 89.253초                                                   | 나머지는 처치로 다음 경계 미관측. 후반 전체를 여러 회차 반복하는 구조는 이 근거에서 외삽한 가설이며 미관측 후반 검증 완료를 주장하지 않음 |
| 시각은 해당 구간의 표본으로 추정 | 12개 로그에서 opener와 두 번째 Thunderspark 이후를 분리한 위치별 하위 중앙값, 0.1초 반올림                                          | 원본 발생 순서와 대상 수를 시간표에 그대로 복제하지 않음                                                                                  |
| 복합 파동·대상별 패킷 요약       | High Volts 뒤 Lightning Bolt, Roar 뒤 Thunderbolt, Fulgurous Fall 뒤 Rush·B7AE Flight 및 다수의 `B75F/B760/B761` 패킷이 함께 관측됨 | 대표 이름으로 예고하고 unknown 표시는 생략. 개별 패킷·대상별 알림·모든 능력 커버리지는 목표가 아님                                        |
| Shock Storm 표시 복원            | 모든 12개에서 `B0F4` source=Shock Storm은 Storm Surge의 577–581ms 뒤. 대표 `#17441`                                                 | 부모 기믹으로부터 0.6초 뒤 Electrifying Flight 표시. helper의 소속을 범용 규칙으로 확정하거나 sync로 사용하지 않음                        |
| 일부 고유한 신호만 sync          | opener의 High Volts·Golden Talons, 후반의 Thunderspark·Storm Surge·High Volts·Golden Talons                                         | 여러 번 나오는 Roar·다중 대상 후속 패킷은 sync로 사용하지 않음. 넓은 Storm Surge 창은 같은 회차의 고유한 `B0F3`로 제한                    |
| 종료·리셋 정책                   | `InCombat=0`, 기존 모드와 같은 ActorControl wipe, ChangeZone                                                                        | 실제 종료 신호를 받으면 취소. FFLogs 보스 처치 기록이 ACT 신호로 자동 변환됐다고 가정하지 않음                                            |

일정의 주요 값은 opener High Volts 11.7초, Roar 23.0/33.8/72.1초, Golden Talons 45.6초, Fulgurous Fall 62.0초, Electrogenetic Force 65.6초다. 후반은 회차 기준 Storm Surge 12.4초, Electrifying Flight 13.0초, High Volts 19.6초, Roar 31.0/42.4/79.7초, Golden Talons 54.2초, Fulgurous Fall 69.5초, Electrogenetic Force 73.1초다. 대안 분기는 근거가 없어 만들지 않았다.

## 평가 기준·결과

대상은 위 8개 이름/ID의 204건이며, 허용 오차는 1.5초로 정했다. fixture의 FFLogs `cast`를 `Ability`로 투영하고 첫 `B0EB`로 시작한다. 매 기록은 해당 보스의 관측된 처치 시각에 종료하며 ACT 종료 신호를 합성해 실제 자료 검증으로 세지 않는다. 결과는 표시 이름과 시각을 비교한 제한된 개발 재생이다.

| 검사                                          | 작성본                   | 기존 자동 초안             |
| --------------------------------------------- | ------------------------ | -------------------------- |
| 시작 신호만 공급                              | 204/204, 최대 오차 245ms | 204/204, 최대 오차 1,248ms |
| 선정한 주요 관측 전체 공급                    | 204/204, 최대 오차 289ms | 204/204, 최대 오차 1,096ms |
| 각 기믹 직전 5초의 모든 관측과 이후 관측 제외 | 192/192, 최대 오차 289ms | 192/192, 최대 오차 1,096ms |
| ambiguous·violation·unresolved                | 위 개발 검사에서 0       | 위 개발 검사에서 0         |

첫 시전 12건은 시작 조건이므로 5초 예고 검사에서 제외했다. 마지막 검사는 정답 신호가 없는 고정 일정의 미래 실행을 확인하며 실제 UI의 5초 전 표시를 측정한 것은 아니다. 기존 초안도 선정한 모든 기믹을 이미 포함했다. 이번 개선은 전체 커버리지 증가가 아니라 표시 정의 49→18개 정리, 첫/후반 구간의 시간 설정과 복구 창이다.

| 합성 제어 검사                  | 확인한 동작                                                                                 |
| ------------------------------- | ------------------------------------------------------------------------------------------- |
| Storm Surge 5초 조기            | 후속 Electrifying Flight·High Volts·Golden Talons를 5초 앞당김                              |
| Storm Surge 12초 지연           | 이후 Golden Talons와 다음 회차를 12초 늦춤. 이미 처리한 Flight·High Volts는 재실행하지 않음 |
| 같은 Storm Surge 중복           | 후속 표시 중복 없음                                                                         |
| 잘못된 source·이른 B0EB 재등장  | 시계 이동·phase 재진입 없음                                                                 |
| 후속 신호 모두 누락             | 250초 이후 미관측 회차의 Flight도 계속 예측. 실제로 그 회차가 일어났다는 검증은 아님        |
| finish·wipe·zone-exit 후 재시작 | 기존 일정 취소, 새 시작을 기준으로 opener 재생                                              |

지연을 확인하기 전 이미 표시한 이벤트의 오류는 소급 복구하지 않는다. 12초 지연 사례의 복구 범위는 다음 미처리 기믹 이후다. ACT 신호 동등성·실제 게임·UI 표시·미선정 능력·범위 밖 신호 경쟁·새 로그의 독립 일반화는 미검증이다. 현재 결과는 개발 자료에 대한 검토 가능한 파일럿이며 실전 확정본이 아니다.

## 재현

저장소 루트에서 실행한다. 보고서와 자동 초안 출력은 새 경로를 사용한다.

```sh
cargo build
mkdir -p out/griffin-check
target/debug/btimeline validate timelines/mistwake-griffin.kdl
bun scripts/verify_authored_griffin.ts target/debug/btimeline out/griffin-check/report.json
cargo test --test authored_griffin
```

원본 파일이 있으면 스크립트가 모든 입력 해시와 fixture 원본 인덱스·source·시각도 확인한다. 원본이 없어도 커밋된 compact fixture로 재생 가능하며, 확인한 원본 수는 보고서의 `raw_inputs_checked`에 기록한다. 기존 8개 자동 초안과의 비교는 다음처럼 재현한다.

```sh
target/debug/btimeline generate logs/loop-mistwake/*.json --mode dungeon -o out/griffin-check/baseline.kdl
bun scripts/verify_authored_griffin.ts target/debug/btimeline out/griffin-check/comparison.json out/griffin-check/baseline.kdl
```

fixture의 `samples`는 `[첫 Thunderspark 기준 ms, 능력 ID(hex), 원본 이벤트 인덱스]`다. `B0F4`만 Shock Storm이고 나머지는 Griffin이다. 재생 결과는 KDL·fixture·선택적 초안의 해시에 연결되며 최종 KDL 실행에는 어떤 sidecar도 필요하지 않다.
