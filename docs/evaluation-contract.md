# P0 평가 계약

고정일: 2026-09-28. 로컬 감사 입력 224개를 기준으로 한다. 이 분할은 현재 세 전투의 회귀 평가용이며 새로운 전투에 대한 범용 성능의 근거는 아니다. 새 전투 로그를 확보하면 별도 목록과 분할을 고정하고 평가한다.

## 상태와 허용 근거

| 표시 상태              | 필요한 근거                                                                                                 | 허용하는 주장                                         |
| ---------------------- | ----------------------------------------------------------------------------------------------------------- | ----------------------------------------------------- |
| `draft`                | 원본 파일과 생성 report의 provenance                                                                        | 편집 가능한 초안. 생성 자체는 검증 통과를 뜻하지 않음 |
| `internally_validated` | YAML 1.2 단일 문서·중복 키 검사, JSON Schema, semantic 검사, 결정적 렌더링, 해당 경로의 원본 신호 재생 통과 | 검사한 입력과 경로에 대한 내부 검증 완료              |
| `runtime_compatible`   | 위 검사와 cactbot parser 통과, 실제 런타임의 진입·리셋·표시 확인                                            | 검사한 런타임 버전과 경로에 대한 호환 검증 완료       |

검사를 실행하지 않았거나 필요한 근거가 없으면 앞 단계에 머문다. 실패는 별도 실패로 기록하며 상태를 올리지 않는다. 독립 재생은 parser 또는 실제 런타임 검사를 대신하지 않는다. [SPEC의 검증 순서](../SPEC.md#validation-and-compatibility)는 JSON Schema → semantic → cactbot parser다. [연구 방침](timeline-research.md#spec-계약과-미검증-사항)은 공식 cactbot의 생성·검증 코드를 연구 입력이나 정답으로 사용하지 않는다. 두 방침의 충돌은 아직 결정되지 않았으므로 `runtime_compatible`을 부여하지 않는다.

## 고정 평가 입력

| 항목      | 계약                                                                                                                                                    |
| --------- | ------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 목록      | [evaluation-inputs.csv](evaluation-inputs.csv)의 224개 경로와 SHA-256. 파일 내용이 바뀌면 같은 평가로 취급하지 않음                                     |
| 묶음      | 기존 [감사 코드](../scripts/audit_timeline_sequences.py)의 `duplicate_pull_candidates` 23쌍은 `candidate_group` 하나로 취급. 나머지는 파일별 묶음       |
| 분할      | 각 `(encounter, difficulty)`에서 `candidate_group`을 사전순 정렬하고 0부터 매긴 index의 매 5번째(index % 5 == 4)를 `holdout`, 나머지를 `train`으로 고정 |
| 기본 평가 | `train`만 생성·정렬·시간 추정에 사용하고 `holdout`은 최종 재생·오차 집계에 사용. 후보 묶음의 양쪽 파일은 같은 분할                                      |
| 보조 평가 | 중복 후보 묶음에 속한 파일을 평가에서 모두 제외한 결과를 별도 집계                                                                                      |
| 해석      | `candidate_group`은 같은 pull의 **후보**이며 확정 동일성이 아님. wipe의 관측 종료 뒤는 미관측. 도달 수와 report 수를 경로별로 함께 공개                 |

### 새 전투 평가 절차

| 단계              | 고정 규칙                                                                                                                                       |
| ----------------- | ----------------------------------------------------------------------------------------------------------------------------------------------- |
| 새 입력 확보      | 전투명·encounter ID와 무관하게 동일 감사 형식으로 파일 무결성을 확인. 코드 수정 없이 처리할 수 있는지 기록                                      |
| 평가 목록 동결    | 생성 정책을 조정하기 전에 경로·SHA-256·입력 그룹·중복 후보를 새 목록으로 고정. 불완전 수집이나 그룹을 가로지르는 후보는 거부                    |
| 같은 전투 내 평가 | 새 그룹의 후보 pull 묶음별 train/holdout을 고정하고 train으로만 시간·경로를 추정. holdout 재생 결과와 도달 수를 별도 보고                       |
| 범용성 판정       | 현재 세 전투는 회귀 사례다. 처음 보는 합성 ID는 입력 독립성만 확인하며, 새 실제 전투 로그의 결과가 나오기 전에는 범용 실전 성능을 주장하지 않음 |

재현: 먼저 `uv run scripts/audit_timeline_sequences.py > /tmp/btimeline-audit.json`을 실행한다. 다음 명령은 감사 당시 SHA-256과 현재 파일을 대조하고, 성공했을 때만 기존 목록을 교체한다.

```sh
(
  tmp=$(mktemp docs/.evaluation-inputs.XXXXXX) || exit
  trap 'rm -f "$tmp"' EXIT
  nu scripts/freeze_evaluation_inputs.nu /tmp/btimeline-audit.json > "$tmp" &&
    chmod 644 "$tmp" &&
    mv "$tmp" docs/evaluation-inputs.csv
)
```

기존 로그의 모든 쌍 비교에는 시간이 걸린다. 감사 결과는 [연구 스냅샷](timeline-research.md#입력과-감사)과 구분해 재실행 날짜를 기록한다. 공식 타임라인·트리거는 학습, 평가, 정답 비교에 사용하지 않는다.

## 검증 체크리스트

| 주장 / 게이트                                         | 확인 방법                                                                                                                                    | 현재 상태                                                     |
| ----------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------- |
| 입력 파일의 수집 완료·참조·시간 범위·능력 사전        | 감사 출력의 `complete`, cursor, eventCount, `out_of_fight`, `missing_cast_abilities`; P2에서 오류 거부                                       | P2 검사 구현, P6에서 고정 목록을 다시 검사                    |
| 같은 pull 후보가 학습과 평가에 동시에 없음            | 고정 목록의 `candidate_group`별 split 단일값 검사                                                                                            | P0 목록에서 확인                                              |
| 목록에 잘못된 입력이 섞이지 않음                      | `freeze_evaluation_inputs.nu`의 완료 상태·파일 해시·후보 멤버·그룹 검사와 합성 오류 입력 검사(`nu scripts/test_freeze_evaluation_inputs.nu`) | P0 검사 통과                                                  |
| 새 전투에 같은 입력·생성 규칙 적용                    | 현재 세 전투에 없는 ID·이름의 합성 입력, 이후 확보한 실제 로그의 별도 holdout                                                                | 합성 생성·재생 통과, 새 실제 전투 범용 성능은 미검증          |
| YAML 구조·시간·jump·regex·출력 안전성                 | P1 Schema·semantic 테스트와 fixture 정확 비교                                                                                                | Rust 회귀 검사 통과                                           |
| 104 시작·초기화된 105 단독 시작, 반복 B528, kill/wipe | P6 원본 신호 재생과 경로별 결과                                                                                                              | [P6 독립 재생](p6-replay.md); 연결 진입·실제 runtime은 미검증 |
| 동시 sync 선택 순서와 window 경계                     | 실제 cactbot parser/runtime의 같은 신호·경계 fixture                                                                                         | 미검증                                                        |
| 같은 시각 exit sync와 forcejump 우선순위              | 실제 런타임의 충돌 fixture                                                                                                                   | 미검증                                                        |
| 리셋 후 타임라인 시계와 105 진입                      | 실제 런타임의 리셋·재진입 실행                                                                                                               | 미검증                                                        |
| 분기 lookahead 예고와 실제 표시                       | 실제 런타임 표시 검사                                                                                                                        | 미검증                                                        |
| 공식 런타임 호환                                      | parser gate 방침 결정 후 파싱과 위 런타임 검사                                                                                               | 보류                                                          |

검증 결과에는 실행 도구·버전, 입력 SHA-256, 대상 경로, 통과/실패/미실행을 남긴다. 미실행은 통과로 간주하지 않는다.

P6 재현은 `nu scripts/evaluate_replay.nu docs/evaluation-inputs.csv out/p6-evaluation --binary target/release/btimeline`으로 실행한다. CSV의 후보 묶음과 분할은 재계산하지 않고 동결된 값을 검사·사용한다. 결과의 `evaluation.json`은 manifest·입력·생성 YAML·생성 report의 SHA-256, train/holdout별 표본·report 수·오류·시간 오차를 보존한다. `nu scripts/test_evaluate_replay.nu target/release/btimeline`은 합성 입력으로 holdout 누출·해시 변경·후보 분할 충돌·덮어쓰기 거부를 검사한다.
