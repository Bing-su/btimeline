# Queen Eternal Castellation 대기·탈출 작성 기록

2026-11-02. [timeline-author skill](../.agents/skills/timeline-author/SKILL.md)로 첫 Castellation 이전의 가변 대기 반복과 실제 신호에 의한 탈출·후속 예고를 작성했다. 전투 전체와 후반 finalHold는 범위 밖이다.

| 항목         | 내용                                                                                                                                                                                                                           |
| ------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| KDL          | [queen-eternal-hold.kdl](queen-eternal-hold.kdl), 22개 이벤트                                                                                                                                                                  |
| 후보 SHA-256 | `00ae9d849f05139ffca88439f8aef45ff3f59905dde79785ec5af2f6068c2c82`                                                                                                                                                             |
| 개발 입력    | [개발 계획](../tests/fixtures/authored/queen-hold-development-plan.json)의 game zone 1202·encounter 1542·difficulty 100, zone page 1/10에서 수집한 14개 파일                                                                   |
| 대표 근거    | `2XD3yzc8PkVYGAq4/114`의 진입 `#5333`, 탈출 `#9002`, 후속 귀환 `#10739`. `JnNP38fbVctXRHrY/30`도 완전한 대기 본문을 지지                                                                                                       |
| 참조         | cactbot `e79ea7f16093be5b80cda0e2d5bc8eb899d6fb4b`의 [Queen Eternal](https://github.com/OverlayPlugin/cactbot/blob/e79ea7f16093be5b80cda0e2d5bc8eb899d6fb4b/ui/raidboss/data/07-dt/trial/queen-eternal.txt)를 로컬 소스로 확인 |
| 보류         | [보류 계획](../tests/fixtures/authored/queen-hold-holdout-plan.json)의 미사용 보고서 8개. zone page 20에서 긴 후보를 선택했으며 이벤트 내용 확인 전에 후보·조건·해시를 고정                                                    |

## 해석·시간·신호

| 결정          | 이유·영향                                                                                                                                    |
| ------------- | -------------------------------------------------------------------------------------------------------------------------------------------- |
| 진입·범위     | 첫 Ability 8F00을 0으로 삼고, StartsUsing 8EFF 탈출 뒤 다음 Ability 8F00 귀환까지 평가. 같은 활성 phase의 귀환 신호가 반복을 재시작하지 않음 |
| 고정 도입부   | Coronation·Aethertithe·Ruthless Regalia와 cone 세 번을 40.7초까지 표시                                                                       |
| 반복 경계     | Prosecution을 반복 첫 이벤트로 두고 54.7초부터 60.6초 주기를 사용. 참조의 Force 시작점보다 경계를 앞으로 옮겼지만 시간표는 같은 진행을 표현  |
| 조기 탈출     | 개발 11개는 약 49.7초에 반복 시작 전 탈출. Prosecution을 phase 직속에 두면 탈출 후에도 남으므로 반복 안에 포함해 취소                        |
| 감시·우선순위 | until은 phase 40.8초부터 감시. HP·FFLogs 종료가 아니라 실제 StartsUsing 8EFF를 사용하고, 미실행 반복 이벤트를 취소                           |
| 반복 보정     | Prosecution과 Royal Domain만 sync. 반복 회차 보정은 phase와 until 감시 시각을 이동시키지 않음                                                |
| 탈출 뒤 진행  | on-exit에서 Virtual Shift·Castellation·Besiegement·Force·귀환을 예고. 첫 Shift sync는 종료 회차가 아닌 phase 시계를 보정                     |
| 표시 요약     | 방향별 Force ID와 동시 Besiegement 패킷은 같은 기믹의 한 파동으로 묶음. 원본 cast/begincast 관측은 재생에 그대로 전달                        |

시간은 개발 표본의 상대 시각을 0.1초로 반올림했다. 개발 자료에는 완전한 대기 본문 2개와 부분 본문 1개가 있지만 다음 회차 시작까지는 관측되지 않았다. 초기 반복 주기 60.6초는 고정 참조를 따른 가설이며, 실제 재반복 확인과 구분한다.

## 개발·합성 검증

| 검사      | 결과                                                                                                    |
| --------- | ------------------------------------------------------------------------------------------------------- |
| 실제 개발 | 14/14 적격·통과, 표시 대응 239건, 조회 225건, 최대 조회 오차 121ms                                      |
| 조회 조건 | 첫 진입은 조회 제외. 탈출 Shift는 시전 시작 관측 후 3초 전, 나머지는 5초 전 조회. 허용 오차 1.5초       |
| 실제 탈출 | 반복 시작 전 11개, 반복 진입 뒤 3개. until 시각 일치·탈출 뒤 hold 표시 없음·phase 재진입 없음           |
| 합성 반복 | 250초 탈출까지 여러 회차를 예고하고 첫 회차를 +5초 보정해 이후 회차까지 이동하는지 확인                 |
| 합성 탈출 | 시작 전/여러 회차 뒤 탈출, 탈출 Shift +2초 보정 후 Castellation 이동, 귀환 신호 이후 반복 재활성화 없음 |

반복의 완전한 본문과 실제 다음 회차 시작 횟수는 각각 `complete_bodies`와 `observed_recurrences`로 분리한다. 다음 회차가 나오기 전 탈출한 자료를 재반복 증거로 세지 않는다. 실제 관측 없는 미래 반복 예고 검사는 합성 결과다.

```sh
cargo build
target/debug/btimeline validate timelines/queen-eternal-hold.kdl
bun scripts/verify_authored_queen_hold.ts target/debug/btimeline tests/fixtures/authored/queen-hold-development-plan.json out/queen-hold-authoring/development-new.json
bun scripts/verify_authored_queen_hold.ts target/debug/btimeline tests/fixtures/authored/queen-hold-holdout-plan.json out/queen-hold-authoring/holdout-new.json
bun run --cwd scripts test
cargo test --test authored_queen_hold
```

원본 입력은 manifest의 경로·해시와 일치해야 하며 출력은 새 경로만 허용한다.

FFLogs cast/begincast→Ability/StartsUsing의 투영 가정이며 실제 ACT·UI·리셋 신호는 미검증이다. 후보를 보류 결과에 맞춰 수정하면 해당 자료는 개발 자료가 되고 새 보류 자료가 필요하다.
