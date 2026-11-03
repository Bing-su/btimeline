# Queen Eternal Castellation 대기·탈출 작성 기록

## Phase 전환으로 이전

`hold`에서 Queen Eternal의 `8EFF` 시전 시작을 받으면 `castellation`으로 전환한다. 접근 일정이 끝나는 40.8초부터 감시하며, 관측보다 넓은 3,600초의 전환 창을 둔다. 후속 일정은 관측 시각을 기준으로 시작한다. 귀환의 `8F00`은 후속 동기화에 쓰이며, 활성 phase가 있을 때 대기 phase를 다시 시작하지 않는다.

실측 22개와 합성 3개의 기존 재생과 비교했다. 21개는 모든 표시 시각이 일치했다. 나머지 4개는 실제 관측에 맞춘 phase 기준점 때문에 첫 Virtual Shift만 4–36ms 달랐으며, 후속 표시는 모두 일치했다. 갱신된 계획의 회귀 평가에서 개발 14개와 이전 보류 자료의 적격 6개가 통과했고, 중복 기술열 2개는 제외됐다. 아래 최초 평가의 해시와 수치는 이전 버전의 기록이다.

## 최초 작성과 평가

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

## 고정 후보의 보류 평가

후보·조건·새 입력 해시를 `04453f8`로 커밋한 뒤 보류 8개 파일을 평가했다. 후보 KDL은 수정하지 않았다. 주요 관측의 상대 시각·ID 열이 같은 2개는 중복으로 제외하고 6개 모두 통과했다. `V1ZXHGMxBqrQ8gvN/16`·`hjTfF1tzMp9BVcRm/16`의 선택 관측 fingerprint가 `6LxYV8WtAC7pyRhj/16`과 같아 별도 근거로 세지 않는다. 모든 변형 업로드의 독립성까지 증명하는 검사는 아니다.

| report/fight          | 탈출 시각 | 표시 대응 | 최대 조회 오차 |
| --------------------- | --------- | --------- | -------------- |
| `rmAQkxKNRMCy68ZD/9`  | 90.225초  | 20/20     | 112ms          |
| `kgAGH4FfrRB3hW9c/90` | 104.391초 | 22/22     | 92ms           |
| `jmaqT9RBtN4FwkXM/16` | 49.730초  | 16/16     | 117ms          |
| `ATtGZKgL1VN7zbBy/55` | 49.681초  | 16/16     | 82ms           |
| `6LxYV8WtAC7pyRhj/16` | 49.685초  | 16/16     | 79ms           |
| `1QXHjgRrp8nDwBYA/5`  | 49.753초  | 16/16     | 140ms          |

보류 범위에서 cast/begincast 관측 5,623건을 전달했다. 표시 106/106건, 사전 조회 100/100건이 대응했고 첫 탈출 Shift 6건은 3초 전, 나머지 94건은 5초 전 조회다. 탈출은 반복 시작 전 4개·진입 뒤 2개이며 모든 재생에서 until 시각 일치·탈출 뒤 hold 표시 없음·잘못된 phase 재진입 없음·실행 진단 없음을 확인했다.

`kgAGH4FfrRB3hW9c/90`에서 추가 완전한 대기 본문 하나를 확인했지만 실제 다음 회차는 여전히 미관측이다. 개발·보류를 합쳐 완전한 본문 3개이며 관측된 재반복 경계는 0개다. 더 느린 자료를 찾으려던 zone page 40/100 탐색은 FFLogs GraphQL 오류로 후보를 확보하지 못했다. **실제 여러 회차 반복 검증은 남아 있으며**, 이번에 확인한 범위는 실제 대기 본문·신호 탈출·후속 예고와 합성 다회차 실행이다. 전투 전체 길이만으로 반복 관측을 주장하지 않는다.

[검증 기록](queen-eternal-hold.validation.json)에 후보·계획·원본 해시, 진입·탈출·귀환 인덱스, 반복 시작과 본문 완성 수, 표본별 원본 인덱스·시각·재생/조회 오차를 보존했다. `sample_columns`가 원본 표본 배열의 열을 설명한다. 실제 데이터의 탈출은 합성한 전투 종료가 아니라 관측된 시전 시작이다.

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
