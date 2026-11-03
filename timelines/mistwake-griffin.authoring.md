# Mistwake Griffin 작성 기록

2026-11-02. [timeline-author skill](../.agents/skills/timeline-author/SKILL.md)의 첫 실제 로그 적용이다. 표시 대상은 dungeon mode의 Thundergust Griffin 한 보스다. 후속 연속 재생에서는 입력에 포함된 앞선 보스·이동 구간도 관측으로 전달해 잘못된 진입·동기화 여부를 검사하며, 그 구간의 표시 일정은 작성하지 않았다.

## 산출물·입력 역할

| 항목             | 값                                                                                                                                                                             |
| ---------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| 최종 KDL         | [mistwake-griffin.kdl](mistwake-griffin.kdl), 18개 이벤트 정의                                                                                                                 |
| KDL SHA-256      | `d2a2d80632bfe330e65589a3e4f7e332c9b485ae6bb5c6e3c6c63ca8da18ab1c`                                                                                                             |
| 개발 코퍼스      | `logs/loop-mistwake/*.json` 8개 + `logs/mistwake-longer/*.json` 4개. 전부 개발 자료                                                                                            |
| 원본 근거        | [griffin-samples.json](../tests/fixtures/authored/griffin-samples.json)에 12개 파일 해시·report/fight·원본 시작/완료 인덱스·선정한 204건의 원본 이벤트 인덱스와 상대 시각 보존 |
| fixture SHA-256  | `b51452bb6a4ed8384275dc93fb858952f307c8ec140a05e66540e9a64adab104`                                                                                                             |
| 대표 경로        | `YQAawtmnD6FRVzPj/80`. 세 번째 Thunderspark까지 관측. 확장 자료 `nfx8gFT3b26kqyLM/11`도 같은 후반 경계를 지지                                                                  |
| 기존 자동 초안   | `out/griffin-authoring/baseline.{kdl,evidence.json}`, 기존 8개 입력으로 재생성. Griffin의 유한 17개 + 반복 32개 = 49개 정의                                                    |
| 초안 KDL SHA-256 | `2a0a9b7c3daa4b797766f0e8243b24673d0e33382712e67ae2554301c0138df8`. 생성 evidence는 수정하지 않음                                                                              |
| 독립 평가        | 아래 후속 평가의 새 보고서 6개 중 적격 4개 통과, 입력 조건 미충족 2개 제외. 기존 개발 12개는 독립 근거로 세지 않음                                                             |

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

### 후속: 지연·누락 후 복구

두 번째 Thunderspark부터의 실제 개발 표본에 −5초 또는 +12초를 더하고, 종료 시각도 같은 양만큼 옮겼다. 신호 누락은 입력 관측에서만 제거하고 평가 대상 기믹은 유지했다. 이 6개 변형을 12개 trace에 적용한 72개 합성 사례이며 새 실제 로그나 독립 검증으로 세지 않는다.

기존 파일의 SHA-256은 `1e43a7efbec38729f14bf33e26755bfc4f1cf6d55d9815b53a5fe893c05c4ce2`였다. +12초 지연에 Storm Surge 누락이 겹치면 High Volts·Golden Talons의 after=8.0 창도 놓쳐 이후를 복구하지 못했다. 두 신호의 후반 회차 창만 after=25.0으로 넓혔다. 회차 내 같은 ID는 한 번이며 High Volts 창 끝 44.6초와 Golden Talons 창 끝 79.2초는 주기 89.3초 안에 있다. opener와 다른 신호의 창은 유지했다. 새 해시는 위 산출물 표에 기록했다.

기믹 5초 전 `forecast`에 작성 이벤트 ID와 시각을 대조한다. 같은 이름의 다른 Roar 위치는 정답으로 세지 않는다. 복구는 마지막 실패 뒤 남은 관측 대상 전체가 맞고, 그 대상이 최소 두 개일 때만 확인한다. 복구 전 실패는 삭제하지 않는다.

다음 표의 수정 전은 자동 초안이 아닌, 위에 해시를 남긴 이전 작성 KDL이다.

| 합성 사례                               | 수정 전 예고 대응 | 수정 후 예고 대응 | 수정 후 복구 확인 trace |
| --------------------------------------- | ----------------- | ----------------- | ----------------------- |
| 5초 조기                                | 72/108            | 72/108            | 12/12                   |
| 12초 지연                               | 60/108            | 60/108            | 12/12                   |
| 5초 조기 + Storm Surge 누락             | 60/108            | 60/108            | 12/12                   |
| 12초 지연 + Storm Surge 누락            | 0/108             | 48/108            | 11/12                   |
| 12초 지연 + Storm Surge·High Volts 누락 | 0/108             | 25/108            | 8/12                    |
| 12초 지연 + 후반 기준 신호 모두 누락    | 0/108             | 0/108             | 0/12                    |

모든 변형의 ambiguous·violation·unresolved는 0이지만, 위 예고 실패는 실제로 남아 있다. 기준 신호가 없으면 지연을 알아낼 수 없으며 정상 예고로 간주하지 않는다. `c6BadvCfAZyFqpQh/1`은 High Volts 복구 후 확인할 대상이 하나뿐이다. Golden Talons만 남긴 경우 이 trace는 해당 신호 전에 종료됐고, `dN2P37qHWt1mFD6R/13`, `FJ3LnT91abvQK4gp/1`, `nfx8gFT3b26kqyLM/1`은 신호가 관측됐어도 그 뒤 대상이 없어 복구 확인에서 제외했다.

대표 trace의 Storm Surge 누락 사례는 두 번째 Roar 136,233ms부터, 두 선행 신호 누락 사례는 Fulgurous Fall 163,350ms부터 남은 예고가 복구됐다. 이미 처리한 표시를 다시 출력하지 않는다. 창 끝의 +25초 신호는 수용하고 그보다 한 틱 늦은 +25.1초 신호는 무시하는 KDL 단독 회귀 검사도 추가했다.

스크립트의 `recovery`에 변형·누락 신호·원본 인덱스·기믹별 query/실행 오차·실제 수용한 sync·복구 시각과 남은 확인 대상 수를 기록한다. 정상 개발 재생 204건과 192건의 조회 성능은 위 결과와 동일하다. 이 합성 검사의 통과가 관측하지 않은 더 큰 지연이나 모든 실제 경로에서의 제어 안전성을 증명하지는 않는다.

### 후속: 조회 시점의 미래 일정 확인

`forecast` 추가 후 검증 스크립트의 `five-second-query`는 정규화된 전체 기록을 전달하되 각 기믹 5초 전을 cutoff로 조회한다. 과거의 제한 관측 재생보다 직접적으로, 후반 정답 관측을 사용하지 않고 해당 시점에 어떤 일정이 예정됐는지 확인한다. 현재 진단과 가상 미래의 제약 위반도 따로 확인한다.

| 대상           | 조회 대응 | 조회 시점 기준 lead_ms | 최대 오차 |
| -------------- | --------- | ---------------------- | --------- |
| 작성본         | 192/192   | 4,876–5,289ms          | 289ms     |
| 기존 자동 초안 | 192/192   | 3,904–6,073ms          | 1,096ms   |

이는 CLI의 조건부 미래 일정 조회 결과다. 실제 UI의 표시 범위·렌더링이나 ACT 입력을 확인한 것은 아니며, 기존 개발 자료를 다시 검사했으므로 독립 평가로 세지 않는다. 작성본·fixture 해시는 위와 동일하다.

### 후속: 전체 시전과 종료 경계

주요 시전만 선별한 입력이 경쟁 신호를 숨기지 않는지 확인하기 위해 같은 12개 개발 원본의 **모든 `cast`**를 `Ability`로 투영했다. 플레이어·일반 몬스터·앞선 보스·helper를 포함하고 source는 원본 actor 이름을 사용했다. 각 원본 첫 이벤트를 0으로 삼고, Griffin 첫 Thunderspark까지의 구간도 건너뛰지 않았다. 이는 투영한 시전 관측의 연속 재생이며 원본의 damage·status 등 모든 타입이나 실제 ACT 패킷을 재생한 것은 아니다.

| 검사                | 결과                                                                                                     |
| ------------------- | -------------------------------------------------------------------------------------------------------- |
| 입력 범위           | 12개 원본의 시전 45,078건, Griffin 진입 전 33,555건 포함                                                 |
| 진입 안전성         | 12개 모두 첫 Griffin Thunderspark에만 phase 진입, 이전 구간의 Griffin 표시 없음                          |
| 주요 기믹 표시      | 204/204 대응, 허용 오차 1.5초                                                                            |
| 5초 전 조회         | 192/192 대응, 최대 오차 289ms; 현재·가상 미래 실행 진단 없음                                             |
| 종료 신호 없는 연장 | 원본의 마지막 시전·Griffin HP=0 중 늦은 시점부터 30초 연장하면 12개 모두 계속 예측, 처치 뒤 표시 총 47건 |
| 합성 종료 신호      | Griffin HP=0 시점에 `InCombat=0`을 추가하면 이전 표시는 동일하고 이후 표시는 모두 취소                   |

HP=0은 원본 damage의 대상이 Griffin이고 `targetResources.hitPoints=0`인지 확인한 평가 경계다. 이를 실제 ACT `InCombat=0` 관측으로 간주하지 않는다. 종료 신호 없는 결과는 실행 계약에 맞지만, 외부 실행기가 종료 신호를 공급하지 못하면 처치 뒤에도 예고가 남는다는 사용자 영향을 보여 준다. 실제 신호의 발생·전달 시각은 미검증이다. KDL·fixture 해시는 변경하지 않았다.

보고서 `continuous`에는 trace별 관측 수·진입 시각·최대 조회 오차·합성 종료 시각·종료 신호가 없을 때 남는 표시 수와 투영 가정을 기록한다. 로컬 원본이 없는 경우 해당 검사는 생략하고 `continuous.skipped_inputs`에 경로를 남긴다. compact fixture만 실행한 결과를 이 전체 시전 검사 완료로 세지 않는다.

### 후속: 새 보고서의 보류 평가

후보 KDL·주요 기믹 ID·5초 전 조회·1.5초 허용 오차·입력 적격 조건과 6개 report/fight를 [평가 계획](../tests/fixtures/authored/griffin-holdout-plan.json)에 고정하고 `0c96082`로 커밋한 뒤 이벤트를 평가했다. 기존 개발 보고서와 겹치지 않는 encounter 4549, difficulty 10, rankings page 2000의 새 보고서들이다. 후보 KDL은 평가 전후 같은 해시이며, 결과에 맞춰 수정하지 않았다.

| report/fight           | 입력 판정                                | 주요 기믹 | 5초 전 조회 | 최대 조회 오차 |
| ---------------------- | ---------------------------------------- | --------- | ----------- | -------------- |
| `NK81VJ9gWnkXy7qR/24`  | 적격·통과                                | 21/21     | 20/20       | 95ms           |
| `P9xGdw2Y3C7hMfH1/5`   | 제외: Griffin HP=0 종료 근거 없음        | —         | —           | —              |
| `LxHcZCv8VTbY7BDk/1`   | 적격·통과                                | 19/19     | 18/18       | 103ms          |
| `hzbZrF19qv3yMpKX/2`   | 적격·통과                                | 17/17     | 16/16       | 163ms          |
| `83DnJkLN7KTqWwPf/8`   | 제외: cast sourceID=39의 actor 이름 없음 | —         | —           | —              |
| `TBp72wVPc9Lrzbtj/113` | 적격·통과                                | 19/19     | 18/18       | 217ms          |

적격 4개에서 종료 경계까지 전체 시전 15,890건을 투영했고, 주요 기믹 76/76건과 조회 72/72건이 대응했다. 네 로그 모두 첫 Griffin Thunderspark에 한 번만 진입했고 그 이전의 표시나 실행 진단은 없었다. 종료 신호는 합성하지 않았다. 원본의 HP=0을 평가 종료 경계로만 사용했으므로 실제 ACT 종료·UI 확인 결과로 세지 않는다.

입력 무결성·원본 actor 이름·Griffin 진입·처치 근거와 기존 개발/선정 로그의 중복을 먼저 검사한다. 보고서 ID와 report/fight 중복 외에 `[상대 ms, 주요 능력 ID]` 열의 해시를 비교해 동일한 시전 기록의 복사를 거른다. 이는 이 자료 범위의 중복 검사이며 모든 변형 업로드의 독립성을 증명하지 않는다. 입력 적격 조건을 충족하지 못한 2개는 예고 성능의 분모에서 제외하되 사유와 원본 해시는 남겼다. 예고 실패·실행 오류를 입력 제외로 바꾸지 않으며, 적격 입력이 0개면 통과하지 않는다.

[평가 결과](mistwake-griffin.holdout-result.json)에 당시 계획·KDL·원본 해시, 원본 기믹 인덱스, 재생·조회 오차, 제외 사유와 집계를 보존했다. 현재 원본 평가는 [공통 Rust 평가기](../docs/AUTHORED_EVALUATION.md)와 [Griffin profile](mistwake-griffin.evaluation.json)로 재현한다. 새 결과를 보고 후보를 고치면 이 코퍼스를 개발 자료로 전환한다.

후속 검사에서 입력 투영의 명시적인 `InputExclusion`만 제외하도록 제한했다. `inspect` 명령 실패·예상하지 못한 예외는 어느 단계에서 발생해도 도구 오류로 남아 전체 평가를 실패시킨다. 정상 재생 하나와 검사 명령 실패 하나를 함께 공급해, 실패한 검사를 제외하여 통과시키지 못하는 CLI 회귀 검사를 추가했다. 같은 원본으로 재현한 적격 네 로그의 결과·집계는 기존 기록과 동일하다. 제외 기록에는 `P9xGdw2Y3C7hMfH1/5`의 진입 `#17332`·마지막 대상 피해 `#21350`, `83DnJkLN7KTqWwPf/8`의 미상 sourceID=39 시전 `#10622`를 추가했다. 개발 fixture에 고정한 원본 해시 불일치는 제외로 완화하지 않는다.

[오프라인 코퍼스 회귀 검사](../tests/griffin_corpus.rs)는 이 결과 파일의 관측 시각·ID를 재사용한다. 별도 fixture를 복제하지 않고 적격 네 로그의 주요 시전만 정규화해, 임시 디렉터리에서 KDL·정규화 기록만으로 5초 전 조회 72건을 실행한다. 평가 계획 해시와 후보 기록의 연결, 1.5초 허용 오차, 실행 진단 없음, cutoff 이하 관측 수를 검사한다. 첫 진입 기믹은 기존 평가와 동일하게 조회 대상에서 제외한다. `cargo test --test griffin_corpus` 또는 전체 `cargo test`로 실행하며 원본 로그·FFLogs 자격 증명·네트워크는 필요하지 않다.

| 검사                      | 확인 범위                                                                                       |
| ------------------------- | ----------------------------------------------------------------------------------------------- |
| 공통 Rust 원본 평가       | 전체 시전의 경쟁 신호·진입 안전성·주요 표시·조회와 입력 적격 조건                               |
| 오프라인 코퍼스 회귀 검사 | 보존한 주요 시전과 현재 KDL의 예고 동작. 원본의 다른 시전·종료 근거·입력 품질은 재검사하지 않음 |

이는 이미 평가한 관측을 사용하는 회귀 검사이며 새 독립 평가가 아니다. 회귀 실패를 보고 KDL을 수정하면 해당 로그는 개발 자료가 되며, 수정 후보의 독립 주장에는 새 보류 자료가 필요하다.

원본 검증을 이전하면서 기존 6개 로그의 report/fight·시각·원본 해시를 확인해 새 manifest에 고정했다. 기존 적격 4개·제외 2개 및 76개 대상의 원본 인덱스·재생/예고 오차가 그대로였다. 개발 12개에서는 적격 9개가 통과했고 시전자 이름을 알 수 없는 3개를 제외했다. 이는 전체 시전 투영 기준이며 이전에 선택 기술만 재생한 12개 결과와 범위가 다르다. 기존 합성 perturbation과 과거 후보 비교 스크립트는 별도로 유지한다.

```sh
target/debug/btimeline evaluate tests/fixtures/authored/griffin-native-regression-plan.json --profile timelines/mistwake-griffin.evaluation.json -o out/griffin-native-regression-new.json
target/debug/btimeline evaluate tests/fixtures/authored/griffin-native-development-plan.json --profile timelines/mistwake-griffin.evaluation.json -o out/griffin-native-development-new.json
```

원본을 새로 수집하려면 FFLogs 자격 증명을 환경에 둔다(`bun`은 로컬 `.env`를 읽는다). 아래 명령은 고정한 report/fight를 사용한다. 수집 시각·보고서 revision 등에 따라 재다운로드 파일 해시가 달라질 수 있으므로 재현 결과의 입력 해시를 함께 확인한다.

```sh
bun -e '
import { execFileSync } from "node:child_process";
const plan = await Bun.file("tests/fixtures/authored/griffin-holdout-plan.json").json();
for (const fight of plan.fights)
  execFileSync("target/debug/btimeline", ["fflogs", fight.report, String(fight.fight), "-o", "logs/griffin-holdout"], { env: process.env, stdio: "inherit" });
'
```

### 실행 명령

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
