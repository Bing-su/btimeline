# Mistwake 전체 보스 작성 기록

2026-11-02. [timeline-author skill](../.agents/skills/timeline-author/SKILL.md)로 세 보스의 대표 기믹을 작성했다. 첫 시전 자체의 사전 예고와 이동 구간 표시는 범위 밖이다. 기존 [Griffin 단독 KDL](mistwake-griffin.kdl)은 보존하고 그 phase를 그대로 포함했다.

| 항목         | 근거·범위                                                                                                                                                                                                                                                                      |
| ------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| 산출물       | [mistwake.kdl](mistwake.kdl), Treno 21·Amdusias 11·Griffin 18개, 총 50개 이벤트 정의                                                                                                                                                                                           |
| 후보 SHA-256 | `c13eb162a03efa629d04062cf517cf1cc791e22a17ba78afc602509c44024fc6`                                                                                                                                                                                                             |
| 개발 입력    | [개발 계획](../tests/fixtures/authored/mistwake-development-plan.json)의 17개 경로·해시. 기존 Griffin 개발 12개와 rankings page 2500에서 새로 수집한 5개                                                                                                                       |
| 새 개발 근거 | `2K1GvNpQhc8XYM6P/19`는 Amdusias의 다음 Thunder IV, `XCWGRwgK41mLt73Z/2`는 Treno 후반 Thunder III·Thunder II·Bedeviling Light까지 관측                                                                                                                                         |
| 참고 구현    | cactbot 커밋 `e79ea7f16093be5b80cda0e2d5bc8eb899d6fb4b`의 [Mistwake](https://github.com/OverlayPlugin/cactbot/blob/e79ea7f16093be5b80cda0e2d5bc8eb899d6fb4b/ui/raidboss/data/07-dt/dungeon/mistwake.txt). 로컬 소스로 확인했으며 해당 자료도 두 보스의 loop 확인을 TODO로 남김 |
| 보류 입력    | [보류 계획](../tests/fixtures/authored/mistwake-holdout-plan.json)의 새 보고서 6개. 기존 개발·Griffin 보류 보고서와 분리하고 내용 평가 전에 후보와 평가 조건을 고정                                                                                                            |

## 표시·동기화 결정

| 결정                            | 근거와 사용자 영향                                                                                                                                            |
| ------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 첫 named mechanic으로 보스 진입 | Treno A93F, Amdusias B118/B11D, Griffin B0EB. 자동 공격·unknown B12D를 제어 신호로 사용하지 않음. 어느 보스에서든 wipe 후 시작 가능                           |
| 보스별 상대 시계                | 다른 보스 첫 시전은 이전 phase를 닫음. 이동 시간은 새 보스 시간표에 포함하지 않음                                                                             |
| Treno Thunder II 요약           | A943 telegraph를 대표 표시로 사용. A944/A945의 대상별 발동은 같은 기믹의 후속 관측이며 별도 표시하지 않음                                                     |
| Amdusias Concerto ID 대안       | B118/B11D는 앞·뒤 배치 차이지만 후속 일정은 같음. 같은 이름으로 표시하고 두 ID를 검색 가능한 field 대안으로 동기화                                            |
| Amdusias 다중 패킷 요약         | Bio II·Thunder IV·Shockbolt 후속 패킷을 대표 시전으로 요약. B11F Thunder의 동시 대상 패킷은 평가에서도 100ms 파동 하나로 묶음. 원본 재생 관측은 삭제하지 않음 |
| Treno 반복                      | opener 뒤 112.8초부터 87.6초 가정. 마지막 opener Earthquake와 8.1초 차이이므로 반복 첫 Earthquake의 before를 5초로 제한. 나머지 기준점은 후속 복구용          |
| Amdusias 반복                   | 첫 Thunder IV 기준 55.7초부터 85.6초. 새 개발 로그에서 55.709→141.336초 간격 85.627초를 관측. 기존 자료의 후반 Bio II도 같은 진행을 지지                      |
| 반복의 한계                     | Treno의 전체 반복·다음 경계와 Amdusias의 여러 완전한 회차는 미관측. 고정 참조와 관측 접미부를 사용한 가설이며 모든 긴 전투의 정확성을 주장하지 않음           |
| 종료·리셋                       | InCombat=0, wipe ActorControl, ChangeZone과 참조의 arena unseal SystemLogMessage 7DE. FFLogs HP=0은 평가 경계로만 사용하며 실제 종료 신호로 변환하지 않음     |

## 검사와 재현

개발 검사에서는 모든 cast를 원본 actor 이름의 Ability로 투영한다. 세 보스의 진입과 HP=0 종료를 모두 요구하며, 없는 구간·비어 있는 actor 이름은 입력 제외로 남긴다. 전체 시전은 재생에 전달하고 주요 기믹만 평가 대상으로 선택한다. 예고 오차 허용은 1.5초이며 각 기믹 5초 전부터 6.5초 범위를 조회한다. 입력 제외와 도구·실행 오류는 [작성 합의](../docs/AGENT_TIMELINE_AUTHORING.md)에 따라 분리한다.

| 개발 검사      | 결과                                                                                                |
| -------------- | --------------------------------------------------------------------------------------------------- |
| 입력 선정·적격 | 17개 중 9개 적격, 8개 제외                                                                          |
| 전체 시전 관측 | 35,347건                                                                                            |
| Treno          | 주요 기믹 114/114, 조회 105/105, 최대 조회 오차 352ms                                               |
| Amdusias       | 주요 기믹 85/85, 조회 76/76, 최대 조회 오차 155ms                                                   |
| Griffin        | 주요 기믹 161/161, 조회 152/152, 최대 조회 오차 289ms                                               |
| 연속 진입      | 적격 9개 모두 Treno→Amdusias→Griffin의 관측 시각과 phase 진입이 일치                                |
| 합성 lifecycle | arena unseal·비전투·wipe·zone exit 각각에 대해 이동 중·최종 종료 후 표시 취소와 세 보스 재시작 확인 |

개발 표본의 위치별 하위 중앙값을 0.1초로 반올림해 초기 시간을 설정했다. 대표 `YQAawtmnD6FRVzPj/80`에서 Treno A93F `#3479`, 후반 Earthquake `#5649`, Amdusias의 기존 첫 주요 시전 이후 진행을 조사했다. 이 파일은 전체 투영 중 비어 있는 actor 이름 때문에 전체 보스 평가에서 제외되며, 조사한 내용은 개발 근거로만 사용한다.

## 고정 후보의 보류 평가

후보·입력 해시·평가 기준을 `72d0557`로 커밋한 후 새 보류 로그의 이벤트 내용을 평가했다. 후보 KDL은 수정하지 않았다. 새 보고서 6개 중 적격 5개가 모두 통과했고, `kdjw64TZtRXAmCxM/32`는 원본 `#11663`의 sourceID=268 이름을 복원하지 못해 제외했다. 기존 개발·Griffin 보류 보고서와의 겹침을 확인했고, 세 보스의 주요 시전 상대 시각·ID 열 fingerprint로 개발 표본·보류 표본 내 동일 기록도 검사했다. 모든 변형 중복 업로드의 독립성을 증명하는 검사는 아니다.

| 보류 검사      | 결과                                                      |
| -------------- | --------------------------------------------------------- |
| 전체 시전·진입 | 24,353건, 적격 5개 모두 세 보스 진입 시각·순서 일치       |
| Treno          | 주요 기믹 68/68, 조회 63/63, 최대 조회 오차 229ms         |
| Amdusias       | 주요 기믹 53/53, 조회 48/48, 최대 조회 오차 304ms         |
| Griffin        | 주요 기믹 108/108, 조회 103/103, 최대 조회 오차 249ms     |
| 합계           | 주요 기믹 229/229, 조회 214/214, 현재·가상 미래 진단 없음 |

보류 자료의 가장 긴 보스 관측은 Treno 137.994초, Amdusias 134.525초, Griffin 215.073초다. Treno의 완전한 후반 회차와 Amdusias의 이후 완전한 회차는 여전히 미확인이다. 전체 던전 로그가 길다고 모든 보스의 반복을 확인한 것으로 보지 않는다.

[검증 기록](mistwake.validation.json)에 개발·보류 계획 해시, 모든 원본 해시·제외 근거, 보스 진입/종료 인덱스, 주요 시전별 원본 인덱스·시각·ID·재생/조회 오차를 보존했다. `sample_columns`가 표본 배열의 열을 설명한다. 전체 시전 재생은 종료 신호를 합성하지 않으므로 처치 후 봉인 해제 신호의 실제 가용성·이동 중 취소는 검증하지 않는다. 위 lifecycle 검사는 명시적인 합성 관측으로 해당 신호를 받았을 때의 동작을 확인한 것이다.

```sh
cargo build
target/debug/btimeline validate timelines/mistwake.kdl
bun scripts/verify_authored_mistwake.ts target/debug/btimeline tests/fixtures/authored/mistwake-development-plan.json out/mistwake-authoring/development-new.json
bun scripts/verify_authored_mistwake.ts target/debug/btimeline tests/fixtures/authored/mistwake-holdout-plan.json out/mistwake-authoring/holdout-new.json
bun run --cwd scripts test
cargo test --test authored_mistwake
```

원본 파일은 manifest 경로에 있어야 하며 해시가 다르면 오류다. 출력은 새 경로만 허용한다. 보류 결과를 보고 후보를 수정하면 그 자료를 개발 자료로 전환하고 새 보류 자료를 확보한다. 실제 ACT·OverlayPlugin·UI, 종료 신호 전달, 모든 미관측 반복 회차는 아직 확인하지 않았다.
