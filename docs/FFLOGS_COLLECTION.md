# FFLogs 수집과 다운로드 계획 v3

`fflogs-collect`는 후보 메타데이터만 조회해 다운로드 순서를 정한다. `fflogs-download`는 계획대로 원본 이벤트를 받고 완료 파일을 검증해 재개한다. 생성기의 입력은 계속 수집된 JSON과 mode이며 수집 계획을 요구하지 않는다.

기본 수집 목적은 `training`이다. 긴 진행 후보에서 전체 경로를 학습하도록 우선하며, 조기 종료·빠른 처치 대조 자료는 `--purpose verification`으로 별도 수집한다. 수집 목적은 후보 선택 정책이며 생성기의 학습/검증 입력을 자동으로 분리하지 않는다. 검증 계획을 다른 파일에 저장하고 학습 계획과 겹치는 report/fight는 별도 검증으로 세지 않는다.

## 명령

일반 토벌전처럼 rankings에 없는 콘텐츠는 실제 게임 zone으로 탐색한다. `--game-zone`은 보고서 탐색과 각 fight 필터에 모두 적용된다. encounter·difficulty도 지정하면 세 조건을 모두 만족해야 한다.

```sh
btimeline fflogs-collect --game-zone 1307 --encounter 1547 --difficulty 100 \
  --max-reports 10 --max-fights 12 -o out/doomtrain.plan.json
btimeline fflogs-download out/doomtrain.plan.json -o logs/doomtrain --limit 6
btimeline fflogs-download out/doomtrain.plan.json -o logs/doomtrain --limit 6
```

```sh
btimeline fflogs-collect --game-zone 1307 --encounter 1547 --difficulty 100 \
  --purpose verification --max-reports 10 --max-fights 12 -o out/doomtrain-check.plan.json
```

보고서를 직접 지정할 때도 같은 zone 필터를 사용할 수 있다. zone을 지정하지 않고 encounter만 지정하면 기존 rankings 탐색을 사용한다. `--partition`·`--region`은 zone 탐색과 함께 사용할 수 없다.

```sh
btimeline fflogs-collect REPORT_CODE --game-zone 1361 --encounter 1548 \
  --difficulty 100 -o out/enuo.plan.json
btimeline fflogs-collect --encounter 101 --difficulty 101 --page 10 --pages 2 \
  --max-reports 12 --max-fights 12 -o out/raid.plan.json
```

FFLogs의 일반·극 난이도가 모두 difficulty 100인 사례가 있다. 이름이나 difficulty만으로 구분하지 않는다. zone이 누락됐거나 다른 fight는 명시적인 `--game-zone` 필터를 통과하지 못한다. `gameZone`은 rankings zone과 다른 게임 내 구역 ID다. [공식 ReportFight 스키마](https://www.fflogs.com/v2-api-docs/ff/reportfight.doc.html)

## 후보 선택 규칙

조회한 모든 보고서의 유효 후보를 비교한 뒤 `--max-fights`를 적용한다. 첫 보고서가 제한을 채웠다는 이유로 다음 보고서를 검사하지 않고 멈추지 않는다. 기본 `training`은 다음 우선순위를 순서대로 비교한다.

| 순서 | 선택 기준                                            | 목적                                               |
| ---- | ---------------------------------------------------- | -------------------------------------------------- |
| 1    | 선택 횟수가 적은 encounter·difficulty·game zone 조합 | 여러 전투를 수집할 때 한 전투에 편중되는 것을 완화 |
| 2    | 긴 기록                                              | 후반 기믹·반복을 포함하는 학습 후보 우선           |
| 3    | 이미 선택한 횟수가 적은 보고서                       | 같은 길이의 후보 사이에서 보고서 다양성 확보       |
| 4    | report code·fight ID                                 | 입력 순서와 무관한 동률 결정                       |

학습용은 처치·전멸 비율을 맞추거나 짧은 기록을 교대로 선택하지 않는다. 따라서 긴 전멸 기록이 짧은 처치 기록보다 먼저 선택된다. 긴 기록이 특정 보고서에 몰려 있으면 그 보고서가 많이 선택될 수 있다.

`verification`은 기존의 다음 대조 자료 선택 정책을 사용한다.

| 순서 | 선택 기준                                                          | 목적                                                |
| ---- | ------------------------------------------------------------------ | --------------------------------------------------- |
| 1    | 이미 선택한 횟수가 적은 보고서                                     | 같은 보고서의 여러 시도보다 독립 보고서를 먼저 확보 |
| 2    | 선택 횟수가 적은 encounter·difficulty·game zone 조합               | 여러 전투를 수집할 때 한 전투에 편중되는 것을 완화  |
| 3    | 해당 조합에서 적게 선택된 처치/전멸 결과                           | 처치와 실패 기록을 함께 확보                        |
| 4    | 나머지 조건이 같으면 처치                                          | 최초 성공 경로를 먼저 확보                          |
| 5    | 보고서별 첫 두 선택은 긴 기록, 다음 두 선택은 짧은 기록, 이후 반복 | 후반 반복의 근거와 짧은 종료 대조 자료를 함께 확보  |
| 6    | report code·fight ID                                               | 입력 순서와 무관한 동률 결정                        |

길이는 양수 `combatTime`이 있으면 그 값을 쓰고, 없거나 0이면 fight의 `endTime-startTime`을 쓴다. 없는 전멸이나 추가 보고서를 만들어 채우지 않는다. 서로 다른 보고서가 서로 다른 업로더의 같은 전투일 수 있으므로 보고서 다양성이 독립 관측의 증명은 아니다.

긴 전멸 기록은 전멸기 도달의 후보이며 확인된 전멸기 기록을 뜻하지 않는다. 길이·HP·마지막 phase만으로 전멸기 도달을 확정하지 않는다. 다운로드 후 관측 기반 도달 범위 판정과 보스별 적응적 추가 수집은 [대표 경로 변경 계획](REPRESENTATIVE_PATH_PLAN.md)의 후속 구현이다.

이 정책은 **조회한 후보 안의 우선순위**다. 첫 페이지에 없는 긴 전투를 찾아냈다는 보장이 없으며 `--page`, `--pages`, `--max-reports`와 예산으로 탐색 범위를 조절한다. 던전 전체 길이가 각 보스의 여러 회차를 보장하지 않는다. pull과 본체 관측을 연결한 보스별 길이 평가·적응적 추가 수집은 남은 작업이다.

## 보존하는 데이터

| 위치                | 필드·자료                                                             | 처리                                                                    |
| ------------------- | --------------------------------------------------------------------- | ----------------------------------------------------------------------- |
| 후보 계획           | gameZone, combatTime, fightPercentage, bossPercentage                 | 원본 nullable 값 보존. 백분율은 임의의 진행률로 재해석하지 않음         |
| 후보 계획           | lastPhase, lastPhaseAsAbsoluteIndex, lastPhaseIsIntermission, hasEcho | 경로·진행·조건을 검토할 메타데이터. null은 그대로 유지                  |
| 다운로드 메타데이터 | maps, boundingBox                                                     | 지도 순서·중복 ID와 좌표 범위를 원본 그대로 보존                        |
| 다운로드 메타데이터 | dungeonPulls                                                          | pull ID·이름·encounter·시작/종료·kill·좌표·지도·NPC report ID 보존      |
| 기존 다운로드       | actor·petOwner·instance 정보, FFLogs phase transitions                | 계속 보존하며 phase나 helper 소속의 실행 규칙으로 자동 확정하지 않음    |
| 기존 이벤트         | `dataType: All`, `includeResources: true`                             | cast·상태·징표·연결·대상 선택·mapchange·사망과 제공된 HP·위치 등을 유지 |

2026-11-02 실제 Clyteum 응답에는 지도 3개·pull 9개가 있었지만 보스 이름의 pull도 `encounterID=0`, `kill=false`였다. 이 값을 보스의 정체나 처치로 확정하지 않는다. Doomtrain은 지도 ID를 재방문하는 순서도 반환했다. `mapchange`와 ACT `MapEffect` 역시 같은 신호로 취급하지 않는다. 이 메타데이터를 채굴·실행 규칙에 연결하는 작업은 별도다.

## 계획·재개 계약과 breaking change

| 항목          | 계약                                                                                                                                                    |
| ------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 계획 버전     | `schemaVersion=3`. v1·v2 계획은 거부하며 `fflogs-collect`로 새 경로에 다시 생성                                                                         |
| 수집 목적     | `purpose=training` 또는 `verification`. 생략·알 수 없는 값·목적과 다른 선택 정책은 거부                                                                 |
| 선택 근거     | 학습은 `selectionPolicy=cohort-long-progress-v1`, 검증은 `report-cohort-outcome-long-short-v1`. 필터 후 `candidateCount`, 선택 순서대로 `fights` 저장   |
| 제한·실패     | `truncated`는 페이지·보고서·fight 제한에 따른 후보 범위 제한을 나타냄. API 오류는 `failures`에 남김. 실패나 제한이 있으면 완전한 코퍼스로 주장하지 않음 |
| 다운로드 검증 | report/fight·encounter·difficulty·시작/종료·kill과 계획의 진행 메타데이터를 비교. 새 다운로드는 이벤트 요청 전에 메타데이터부터 비교                    |
| 원본 파일     | 수집 JSON의 schemaVersion은 1을 유지. 기존 파일도 생성·분석에 계속 사용 가능                                                                            |
| 재개          | 완료 파일의 유효성·계획 일치를 확인한 뒤 건너뜀. 불일치 파일을 덮어쓰지 않음                                                                            |

기존 원본 JSON의 형식은 바꾸지 않는다. v3 계획과 메타데이터가 일치하는 완료 파일은 그대로 재사용한다. 불일치 파일은 보존하고 다른 다운로드 디렉터리에 수집한다. 계획에서 관측된 메타데이터가 바뀐 경우에도 계획을 다시 검토·생성한다. `--max-requests`, `--max-points`, `--max-bytes`, `--limit` 예산은 계속 적용된다. 상태 파일은 원본 로그 디렉터리 밖의 `*.plan.download.json`에 둔다.

## 기존 v2의 실제 API 확인 기록

2026-11-02 공식 GraphQL introspection과 실제 보고서로 필드 지원·반환을 확인했다. API 응답과 계획·상태·inspect 결과는 로컬 `out/validation/collection-v2/`, 파일 경로와 SHA-256은 [입력 명세](collection-v2-inputs.csv)에 기록했다.

| 콘텐츠         | 받은 fight | 이벤트 | zone | 확인                                         |
| -------------- | ---------: | -----: | ---: | -------------------------------------------- |
| Doomtrain 일반 |          2 | 29,956 | 1307 | zone 탐색·필터·계획 비교·다운로드·inspect    |
| Enuo 일반      |          2 | 37,890 | 1361 | 위와 동일, 재개 시 2개 건너뛰고 다운로드 0개 |
| Clyteum        |          1 | 17,398 | 1345 | 지도 3개·pull 9개 보존, inspect 통과         |

이 작은 실제 다운로드 표본은 모두 처치 기록이다. 전멸 균형·짧은 기록 선택은 합성 회귀 검사로 확인했다. 일반·극 같은 difficulty의 zone 구분, 모든 보고서를 비교한 뒤 제한 적용, stale zone에서 이벤트 요청 0회, 순서 독립성과 고유성 property, v2 snapshot도 검사한다. 추가 메타데이터 확보 자체가 반복 생성 품질의 개선이나 실제 ACT 검증을 뜻하지 않는다.

v3는 같은 API와 원본 저장 경계를 사용한다. 긴 진행 우선·combatTime 우선·검증용 기존 정책·입력 순서 불변성과 고유성·목적과 정책 일치·v1/v2 거부·v3 snapshot을 로컬 테스트로 검사한다. v3 정책으로 실제 전멸기 로그를 확보했다는 주장은 아직 하지 않는다.
