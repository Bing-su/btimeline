# FFLogs 기반 보편 타임라인 자동생성 연구

조사 기준일: **2026-09-26**. 로컬 `logs/**/*.json` 224개와 [SPEC v1](../SPEC.md)를 기준으로 정리한 연구 문서다. 현재 CLI는 수집기이며 자동생성기는 아직 구현되지 않았다. 수집 방법은 [README](../README.md), 실제 요청 설정은 [수집 코드](../src/fflogs/client.rs)를 따른다.

공식 cactbot의 타임라인·트리거·생성/검증 코드를 입력이나 대조 정답으로 사용하지 않는 연구 방침을 유지한다. 이 문서의 로그 관측, 생성 정책, 합성 실험은 각각 근거의 범위가 다르며, 보편 모델 생성이나 런타임 호환성 검증이 완료됐다는 뜻은 아니다.

| 읽을 내용                         | 위치                                            |
| --------------------------------- | ----------------------------------------------- |
| 핵심 판단과 미검증 사항           | [결론](#결론)                                   |
| 표본·무결성·비교 지표             | [입력과 감사](#입력과-감사)                     |
| 전투별 수치와 원본 근거           | [전투별 관측](#전투별-관측)                     |
| 정규화·정렬·분기 일반화·SPEC 변환 | [생성 설계](#생성-설계)                         |
| 라이브러리 후보와 합성 실험       | [OSS 검토와 실험](#oss-검토와-실험)             |
| 작업 순서와 추가 수집             | [구현 순서와 완료 기준](#구현-순서와-완료-기준) |

## 결론

**생성 방향은 정규화 → 앵커 구간 정렬 → 경로 보존 → 블록 상대시간 추정 → SPEC 컴파일이다.** R12S는 난이도 101의 104·105를 하나의 출력 타임라인으로 다루되 구간별 정렬·시간 통계를 유지한다. 페이즈 전환은 관측 신호를 기다리는 긴 window와 sync jump로 표현한다.

| 판단                                   | 관측 근거                                            | 생성 정책 / 남은 검증                                                      |
| -------------------------------------- | ---------------------------------------------------- | -------------------------------------------------------------------------- |
| wipe는 관측 중단                       | 224개 중 clear 67, wipe 157                          | 종료를 경로 끝·기믹 생략으로 학습하지 않음                                 |
| R12S는 체크포인트를 공유하는 연속 전투 | 사용자 설명과 같은 보고서의 104 완료→105 재시작 관측 | 하나의 파일에 두 진입 경로를 제공. 초기화된 시계의 105 진입 sync 검증 필요 |
| R12S 후반의 네 선택 조합 유지          | 105에서 두 선택에 도달한 파일 55개                   | 도달 분모를 구분. helper 후속 의존성은 추가 검증                           |
| 후반 반복은 유한하게 보존              | B533 두 회차 4개, B537 시작 3개·완료 2개             | B533 두 회차→B537을 펼침. 시전 중 clear는 관측 종료로 처리                 |
| Dancing Mad 후반 wipe 분석 가능        | 5페이즈 도달 20개 중 clear 12, wipe 8                | 확보한 wipe로 시간·종료 신호 분석                                          |
| Clyteum 반복 출구 근거 부족            | 15개 모두 clear, 첫 순서 대안 9:6                    | 전환 직전·후반 wipe 추가 확보                                              |

미관측 조합은 알려진 선택 슬롯의 대안을 조합해 기본 지원한다. 관측된 의존성이 있으면 필요한 선택 이력을 보존하고, 직접 확인한 금지 규칙이 있을 때만 조합을 제외한다. 새 기믹이나 무한 반복을 추정해 추가하지는 않는다. 세부 기준은 [분기 조합 정책](#분기-조합-정책)을 따른다.

## 입력과 감사

### 표본과 무결성

생성 JSON은 문서에 중복 저장하지 않는다. 아래 수치는 조사 기준일의 스냅샷이며, 파일별 상세 근거는 현재 로그와 감사 명령으로 확인한다. 과거 표본 집계는 현재 집계로 대체했다.

```sh
python3 scripts/audit_timeline_sequences.py > /tmp/btimeline-audit.json
```

[스크립트](../scripts/audit_timeline_sequences.py)는 파일별 수집·이벤트 무결성, boss begincast trace, 선택 도달·후반 시전, 페이즈 상대시각을 기록한다. 전투별 모든 쌍을 비교하며 104·101의 990쌍, 105·101의 6,441쌍, 1085·100의 1,225쌍, 4551·10의 105쌍이다. 비교는 구조 탐색 지표이며 완성된 생성기의 정렬·시간 검증이 아니다.

| 전투 / encounter·difficulty | 파일 | 보고서 | clear / wipe | 이벤트 합계 | 길이(초)         | 적 비일반공격 cast/파일 | boss begincast/파일 | 페이지/파일 |
| --------------------------- | ---: | -----: | ------------ | ----------: | ---------------- | ----------------------- | ------------------- | ----------- |
| Lindwurm / 104·101          |   45 |     10 | 14 / 31      |     537,591 | 21.222–449.161   | 1–319                   | 1–19                | 1–3         |
| Lindwurm II / 105·101       |  114 |     19 | 26 / 88      |   1,340,523 | 15.549–559.970   | 0–300                   | 1–39                | 1–3         |
| Dancing Mad / 1085·100      |   50 |     12 | 12 / 38      |   1,671,268 | 16.086–1137.731  | 0–729                   | 1–88                | 1–7         |
| Clyteum / 4551·10           |   15 |     15 | 15 / 0       |     268,765 | 734.004–1246.395 | 138–241                 | 30–48               | 2–3         |

총 이벤트는 **3,818,147개**다. 위 값은 원본 파일을 모두 포함한다. `(report.code, fight.id)` 중복은 없고, 224개 모두 `collection.complete=true`, 최종 cursor null, eventCount 일치다. 전투 범위 밖 이벤트, 적 cast의 능력 사전 누락 및 `(timestamp, sourceID, sourceInstance, abilityGameID)` 중복은 없다. 동일 전투·난이도가 같은 패치임을 증명하지는 않는다. [감사 명령](../scripts/audit_timeline_sequences.py)

| 전투                   | timestamp 역전 파일 / 횟수 | headmarker | tether | targetabilityupdate |
| ---------------------- | -------------------------- | ---------: | -----: | ------------------: |
| Lindwurm / 104·101     | 7 / 8                      |        805 |    235 |                  67 |
| Lindwurm II / 105·101  | 4 / 5                      |        183 |  5,093 |                   2 |
| Dancing Mad / 1085·100 | 10 / 14                    |      2,555 |  2,065 |                 286 |
| Clyteum / 4551·10      | 8 / 11                     |        422 |      0 |                 375 |

추가 신호는 source 적 필터 없이 전체 파일에서 센 값이다. 분석 입력은 timestamp 안정 정렬하고 동시 실행·instance 개수를 보존한다. ID·언어·시간의 정규화는 [입력 시간 의미](#시작완료-및-입력-시간-의미), network sync 대응의 미검증 사항은 [SPEC 계약](#spec-계약과-미검증-사항)에 정리했다.

### 시퀀스 비교 지표와 해석

| 전투                   | 전체 파일 쌍 일치율 범위 | 전체 대응 시각 차이(초) | clear끼리 일치율 / 시각 차이(초) |
| ---------------------- | ------------------------ | ----------------------- | -------------------------------- |
| Lindwurm / 104·101     | 5.3–100.0%               | -0.756–+29.803          | 83.3–100.0% / -0.734–+29.803     |
| Lindwurm II / 105·101  | 2.6–100.0%               | -9.696–+9.238           | 84.6–100.0% / -1.050–+1.128      |
| Dancing Mad / 1085·100 | 1.1–100.0%               | -63.141–+68.653         | 84.1–94.3% / -42.621–+43.033     |
| Clyteum / 4551·10      | 60.4–97.4%               | -348.699–+490.811       | 60.4–97.4% / -348.699–+490.811   |

일치율은 `SequenceMatcher(autojunk=False)`의 matched / 두 trace 최대 길이다. 짧은 wipe의 낮은 전체 일치율은 모델 오류율이 아니고, 100% 일치도 짧은 공통 prefix만 비교한 결과일 수 있다. 반복 토큰 오대응도 포함될 수 있다. 따라서 낮은 일치율이나 시각 차이를 그대로 분기 발견·window 폭으로 쓰지 않는다. wipe는 마지막 정상 anchor까지의 prefix 또는 구간만 비교하며 이후는 미관측으로 처리한다. [감사 명령](../scripts/audit_timeline_sequences.py)

## 전투별 관측

### R12S: 연속 전투와 체크포인트 진입

104 Lindwurm과 105 Lindwurm II는 체크포인트를 사이에 둔 하나의 전투로 다룬다. 사용자 설명에 따르면 체크포인트에 도달하면 보통 전멸한 뒤 105부터 재시작한다. 따라서 encounter ID는 **구간 식별자**이며 출력 파일 분리 기준으로 고정하지 않는다.

104의 45개(clear 14/wipe 31)와 105의 114개(clear 26/wipe 88)는 구간별로 정렬한 뒤 하나의 SPEC 타임라인에 컴파일한다. 104 clear는 전체 전투 clear가 아니라 전반 구간·체크포인트 완료다. 이후의 의도적 전멸은 체크포인트 상실이나 104 재수행을 의미하지 않는다. 입력 감사의 clear/wipe 수치는 FFLogs fight별 플래그 집계다.

#### 같은 보고서의 연결과 재시작 관측

| 보고서           | 앞 구간            | 뒤 구간                   | 104 종료→105 시작 | 확인할 수 있는 것                                                                |
| ---------------- | ------------------ | ------------------------- | ----------------: | -------------------------------------------------------------------------------- |
| cfjArQtVb3NgFZWP | 104 fight 21 clear | 105 fight 22 wipe         |          97.323초 | 104 완료 뒤 105 pull 시작; 그 사이 전멸·재시작 과정은 파일 범위 밖               |
| tjGrJ4LqaXbRk9AT | 104 fight 5 clear  | 105 fight 6 wipe          |          91.224초 | 104 완료 뒤 105 pull 시작; 그 사이 전멸·재시작 과정은 파일 범위 밖               |
| bpNkT471KdWjzHXh | 104 fight 1 clear  | 저장된 105 fight 11 clear |       2,960.592초 | 같은 보고서에 두 구간이 존재. 중간 fight 미수집이라 직접 연결 시간으로 쓰지 않음 |
| NzcYQ9XPT2W47Vhj | 104 fight 7 clear  | 105 fight 8 wipe          |         100.340초 | 인접 fight의 체크포인트 후 시작; 중간 과정은 미수집                              |
| RYVqaCf3zZyc8TpF | 104 fight 60 clear | 105 fight 61 wipe         |         108.484초 | 인접 fight의 체크포인트 후 시작; 중간 과정은 미수집                              |
| NzcYQ9XPT2W47Vhj | 104 fight 3 clear  | 저장된 105 fight 5 clear  |         169.625초 | fight 4 미수집. 직접 전환 간격으로 사용하지 않음                                 |
| 4kz6WLqPGAJ2p8xg | 104 fight 1 clear  | 105 fight 2 wipe          |         114.131초 | 인접 fight의 체크포인트 후 시작                                                  |
| YVPmpQrvfKN4ATWz | 104 fight 10 clear | 105 fight 11 wipe         |          91.975초 | 인접 fight의 체크포인트 후 시작                                                  |
| 4kz6WLqPGAJ2p8xg | 104 fight 10 clear | 저장된 105 fight 12 wipe  |         162.890초 | fight 11 미수집. 직접 전환 간격으로 사용하지 않음                                |
| YVPmpQrvfKN4ATWz | 104 fight 1 clear  | 저장된 105 fight 3 wipe   |         160.747초 | fight 2 미수집. 직접 전환 간격으로 사용하지 않음                                 |

cfj…는 저장된 105 fight 22/24/25/26/28/29/30/31/32의 wipe 뒤에 fight 33 clear가 이어지고, tj…는 105 fight 6–11의 wipe가 이어진다. 저장된 반복 105 구간 사이에 104 재수행은 나타나지 않는다. 이는 사용자 설명의 체크포인트 재시작과 일치하는 관측이다. 보고서의 미수집 fight까지 모두 설명한다는 뜻은 아니다. [cfj 104](../logs/r12s/cfjArQtVb3NgFZWP_21.json), [cfj 첫 105](../logs/r12s/cfjArQtVb3NgFZWP_22.json), [tj 104](../logs/r12s/tjGrJ4LqaXbRk9AT_5.json), [tj 첫 105](../logs/r12s/tjGrJ4LqaXbRk9AT_6.json)

Nzc…에서는 104 fight 7 clear 뒤 105 fight 8–18의 wipe가 이어지고, RYV…에서는 104 fight 60 clear 뒤 105 fight 61–67의 wipe와 fight 68 clear가 이어진다. 반면 Nzc…의 105 fight 5 clear 뒤에는 104 fight 6 wipe/fight 7 clear가 다시 나온다. 따라서 전체 클리어 뒤 시작한 다음 시도의 104를 누락이나 체크포인트 오류로 판단하지 않고 별도 세션 흐름으로 보존한다. [Nzc 104](../logs/r12s/NzcYQ9XPT2W47Vhj_7.json), [Nzc 재시작](../logs/r12s/NzcYQ9XPT2W47Vhj_8.json), [RYV 104](../logs/r12s/RYVqaCf3zZyc8TpF_60.json), [RYV 재시작](../logs/r12s/RYVqaCf3zZyc8TpF_61.json)

위 간격은 `fight105.startTime - fight104.endTime`이며 통상적인 전멸·부활·준비·재시작 시간이 포함될 수 있다. 숫자만으로 그 과정의 개별 시점을 판정하지 않는다. 동일 report의 시간 원점을 사용한다. 각 파일은 해당 fight 범위의 이벤트만 저장하므로 인접 fight에서 91.224–114.131초인 간격의 컷신·체크포인트·전투 진입 신호는 현재 원본에 없다. 이 간격을 고정 duration이나 자동 forcejump 시각으로 쓰지 않는다. 전멸·리셋 동작 자체를 검증하려면 104 종료부터 첫 105 진입까지의 All events가 유용하다. 후반 진입은 기존 105 로그의 독립 anchor로 분석할 수 있으므로, 경계 이벤트 확보를 본문 생성의 선행 조건으로 고정하지 않는다.

#### 하나의 SPEC 타임라인으로 생성하는 정책

| 진입 경우                          | 출력 / 분석 정책                                          | 검증                                                                                         |
| ---------------------------------- | --------------------------------------------------------- | -------------------------------------------------------------------------------------------- |
| 104부터 시작                       | 104 진입 anchor와 전반 이벤트 배치                        | 난이도·구간별 상대시간 재현                                                                  |
| 104 체크포인트 완료 후 전멸·재시작 | 재시작한 타임라인에서도 105 진입 sync로 후반 label에 진입 | 전멸 후 초기화된 시계에서 후반 sync가 활성화되는지 확인. 준비 간격은 시간표에 누적하지 않음  |
| 이후 105에서 다시 전멸·재시작      | 매 pull마다 104 수행 없이도 같은 후반 label에 진입        | 이전 pull의 가상 시각·분기 상태를 이어받지 않음. 105 단독 pull을 전반 생략으로 학습하지 않음 |
| 이후 후반 진행                     | 두 진입 경로가 같은 105 상대시간 모델을 사용              | 104 진행시간·대기시간을 후반 시각에 누적하지 않음                                            |

105의 114개 모두 첫 boss begincast가 B528이며 pull 기준 9.255–18.951초다. 독립 후반 진입 anchor 후보지만 B528은 뒤에도 반복되므로, ID만으로 전 구간에서 활성화되는 sync를 만들지 않는다. source와 진입 문맥, 활성 window, 이미 진입한 후의 반복 신호 충돌을 확인해야 한다. 104 도달 후 재시작과 이후 105 재시작 모두에서, 초기화된 타임라인 시계가 해당 진입 sync를 수신할 수 있는지 검증한다. 하나의 파일을 쓴다는 이유로 이전 pull의 시계를 계속 실행하지 않는다. FFLogs에 없는 체크포인트 전용 네트워크 필드를 창작하지 않는다.

### R12S 후반의 선택과 반복

아래는 난이도 101·encounter 105의 **114개 파일만** 대상으로 한다. 104 파일은 두 선택의 미도달 분모에 넣지 않는다.

| 첫 선택 B52E/B52F | 두 번째 B52B/B52C | 두 선택 도달 파일 | 그중 clear |
| ----------------- | ----------------- | ----------------: | ---------: |
| Near              | Near              |                14 |          6 |
| Near              | Far               |                16 |          4 |
| Far               | Near              |                14 |          8 |
| Far               | Far               |                11 |          8 |
| 합계              |                   |                55 |         26 |

첫 선택 도달은 75개, 둘째는 55개다. 첫 선택만 관측된 20개와 어느 선택도 관측되지 않은 39개는 조합 분모에서 제외한다. 네 조합이 있다는 사실만으로 helper 독립성이나 게임 규칙의 완전성을 증명하지 않는다. 평가는 보고서 묶음 또는 실제 세션 단위로 분리한다.

첫 선택 시작은 154.518–155.596초, 둘째는 223.612–224.641초다. 다음 boss 시작이 관측된 경우 간격은 첫 슬롯 26.328–26.525초, 둘째 8.119–8.167초로 여전히 안정적이다. 후속 anchor 전에 끝난 wipe는 간격 통계나 “후속 없음” 경로로 학습하지 않는다. [파일별 선택·간격](../scripts/audit_timeline_sequences.py)

이전 소표본의 helper 순서 fingerprint는 동일 선택 내에서도 달랐다. 전체 helper 정렬은 아직 수행하지 않았으므로, 시작/완료·동시묶음·instance 정렬을 검증한 뒤 동일 후속 흐름이면 ID 배열로 합치고 의존성이 있으면 분기 블록을 유지한다.

#### B533 두 회차와 B537 종료 관측

B533은 10개 파일에서 관측됐다. 6개는 한 회차, 4개는 두 회차다. 두 회차 이후 B537 시작까지 도달한 파일은 **3개(2 wipe/1 clear)**이고, B537 완료는 두 wipe에서만 관측됐다. 후반 최대 길이는 559.970초이며, 별도 보고서에서도 같은 후반 경로와 시전 중 clear가 관측됐다.

| 원본                                                  | 결과 / 길이       | B533 시작→완료(초)               | B537 시작→완료(초)           |
| ----------------------------------------------------- | ----------------- | -------------------------------- | ---------------------------- |
| [Nzc fight 5](../logs/r12s/NzcYQ9XPT2W47Vhj_5.json)   | clear / 535.366초 | 510.452→515.446, 526.731→531.710 | 시작 전 종료                 |
| [Nzc fight 10](../logs/r12s/NzcYQ9XPT2W47Vhj_10.json) | wipe / 559.970초  | 510.367→515.346, 526.587→531.562 | 544.371→554.338              |
| [4kz fight 3](../logs/r12s/4kz6WLqPGAJ2p8xg_3.json)   | wipe / 559.878초  | 510.275→515.265, 526.528→531.517 | 544.344→554.318              |
| [YVP fight 14](../logs/r12s/YVPmpQrvfKN4ATWz_14.json) | clear / 548.942초 | 510.937→515.927, 527.196→532.187 | 545.015→완료 없음(전투 종료) |

B533 두 회차의 시작 간격은 네 파일에서 16.220–16.279초다. B537까지 도달한 세 파일에서는 두 번째 B533 시작 후 17.784–17.819초에 B537이 시작한다. B537 시작은 544.344–545.015초이며, 두 wipe의 실제 완료는 554.318–554.338초다. 후반 모델은 개별 pull 시각 평균보다 B533 두 번째 회차 등의 국소 anchor 기준 상대시간으로 추정하는 것이 적합하다.

B533과 B537은 관측 이름이 모두 Arcadian Hell이지만 ID와 시전바 duration이 다르다(B533 4.7초, B537 9.7초). 실제 B537 시작→완료 간격도 9.967–9.974초다. 이름으로 합치지 않고 시작·완료 시각과 시전바 duration을 각각 보존한다.

두 wipe는 모두 B537 완료 시각에 `calculateddamage` 8행을 기록하고, Nzc에서는 557.635–557.769초, 4kz에서는 557.606–557.651초에 death 8행이 이어진다. 두 파일 모두 별도 B537 `damage` 행은 없다. 같은 완료→피해→전멸의 시간 경로가 두 보고서에서 재현되어 종료 기믹 후보의 근거가 강화됐다. 다만 로그의 시간 관계만으로 게임의 enrage 조건·무한 반복 여부를 확정하지 않는다. 세 파일의 boss begincast에서는 B537 이후 새 행동이 관측되지 않았다.

YVP fight 14는 B537 시작 후 3.927초 만에 clear하여 완료·피해가 관측되지 않았다. 수집은 완료됐고 전투 자체가 먼저 종료된 경우다. 이를 B537 생략 분기, 누락 이벤트 또는 별도 공격으로 분류하지 않는다. 로그별 시작/완료 대응에는 완료 없음과 kill 종료를 기록하고, 실제로 없는 완료 시각을 생성하지 않는다. 보편 모델의 B537 완료 예측은 다른 두 wipe의 관측 근거로 유지하되 clear 재생에서 그 이벤트의 미발생을 오류로 세지 않는다.

생성 시 **B533 두 회차→B537**을 유한하게 펼쳐 보존하는 정책을 적용한다. B537 시작 sync는 시전 중 clear에서도 관측 가능하지만 완료 sync는 도달한 wipe에서만 관측된다. 예고·duration·동기화 시점을 구분하고, 실제 source·window 및 반복 ID 충돌을 검사해야 한다. 두 회차 관측을 무한 loop로 압축하지 않으며, 짧은 clear의 접미부 부재를 선택적 스킬 생략으로 학습하지 않는다.

### Dancing Mad와 Clyteum

Dancing Mad 50개는 clear 12개/wipe 38개이며, `Yj8wrgHzZWBDmvc7` 한 보고서의 25개 pull과 `4cnjwGdZq9PbKVMN`의 9개, `J34bkTLxrvgdpGXj`의 7개가 포함된다. 아래 도달 수는 실제 `phaseTransitions`에 해당 진입이 기록된 파일 수다.

| 페이즈 진입 | 도달 파일 | pull 기준 최소–최대(초) | 직전 페이즈 길이 최소–최대(초) |
| ----------- | --------: | ----------------------- | ------------------------------ |
| 1           |        50 | 0.000–0.000             | —                              |
| 2           |        36 | 208.359–209.318         | 208.359–209.318                |
| 3           |        31 | 428.050–429.267         | 219.236–220.196                |
| 4           |        24 | 724.470–739.091         | 295.922–310.611                |
| 5           |        20 | 887.625–902.292         | 162.963–163.567                |

3→4 전환의 길이 변동은 **14.689초**다. 긴 전환 window와 jump를 산정할 때의 관측 근거로 사용한다. 도달하지 않은 wipe를 페이즈 생략으로 해석하지 않는다. `phaseTransitions` 자체는 네트워크 sync가 아니므로 실제 전환 cast·targetability 등의 신호를 대응시키고, 그 시점에 보정된 가상 시계를 기준으로 window를 결정한다.

5페이즈 도달 wipe는 Yj8…의 fight 9/12/20/25에 더해 4cn…의 fight 3/4/7과 J34…의 fight 15까지 **8개**다. [J34… fight 15](../logs/dancingmad/J34bkTLxrvgdpGXj_15.json)는 1,137.731초, `kill=false`, fightPercentage 0.07, bossPercentage 0.26으로 다른 긴 wipe 1,131.249초보다 더 길다. kill 여부와 진행도는 별개로 처리한다. 낮은 HP나 길이만으로 enrage·종료 원인을 판정하지 않고 후반 실제 이벤트·종료 직전 신호를 분석한다.

Clyteum은 15개 모두 clear이고 각 25개의 targetabilityupdate가 있다. 첫 C3FF/C400 순서는 C3FF→C400 9개, 반대 6개다. 두 ID의 합계 등장 수는 3회인 파일 13개, 4회 1개, 7회 1개다. boss 역할 19496/19500/19519와 전환 문맥을 활용한다. 긴 로그와 반복 회차는 있지만 wipe·확인된 반복 출구는 여전히 부족하다.

## 생성 설계

### 시작·완료 및 입력 시간 의미

FFLogs `timestamp`와 `fight.startTime`은 보고서 기준 밀리초다. `report.startTime`은 UNIX 시각이므로 상대시간 계산에서 빼지 않는다. [BeginCastEvent](https://www.fflogs.com/scripting-api-docs/ff/interfaces/RpgLogs.BeginCastEvent.html), [ReportFight](https://www.fflogs.com/v2-api-docs/ff/reportfight.doc.html)

같은 actor·instance·ability의 시작과 완료를 순서대로 대응시키되 둘의 시각을 별도로 유지한다. 예를 들어 시작 11.148초, 완료 16.105초라면 Ability 행은 16.1초이고, 시작 행은 11.1초다. 관측 간격 4.957초와 시전바 duration 4.7초는 서로 다르다. 취소되거나 대응 완료가 없는 경우 완료 행을 창작하지 않는다. SPEC duration은 표시 지속시간이며 cast 완료 행에 시전바 duration을 자동 복사하지 않는다. [SPEC](../SPEC.md)

Scripting API는 필드 의미 확인용이며 GraphQL 원시 JSON과 형태가 다를 수 있다. 현재 원시 필드 `melee`/`packetID`/`extraAbilityGameID`는 문서의 `isMelee`/`packetId`/`appliedByAbility`와 표현이 다르다. 문서의 객체를 원시 JSON 스키마로 그대로 사용하거나 필드 이름을 임의 변환하지 않는다. report-local sourceID는 보고서 간 병합 키가 아니며 같은 이름의 Boss와 NPC helper도 별도로 보존한다. [CastEvent](https://www.fflogs.com/scripting-api-docs/ff/interfaces/RpgLogs.CastEvent.html), [ApplyDebuffEvent](https://www.fflogs.com/scripting-api-docs/ff/interfaces/RpgLogs.ApplyDebuffEvent.html), [ReportActor](https://www.fflogs.com/v2-api-docs/ff/reportactor.doc.html)

`masterData.lang`은 원본 로그 언어이며 번역된 이름 언어가 아니다. 보고서 간 비교는 actor.gameID·abilityGameID를 사용한다. headmarker 난독화 ID는 pull 간 숫자 동일성을 가정하지 않고 별도 정규화한다. [HeadMarkerEvent](https://www.fflogs.com/scripting-api-docs/ff/interfaces/RpgLogs.HeadMarkerEvent.html)

### 단일 로그의 최소 초안

이 단계는 **단일 로그의 직선 초안**을 만드는 절차다. 다중 로그의 분기·반복 모델은 다음 절에서 별도로 구성한다.

| 순서 | 처리                                                           | 이유                                         |
| ---: | -------------------------------------------------------------- | -------------------------------------------- |
|    1 | report/fight/event 배열, 숫자·범위·참조 ID 검증                | 잘못된 입력을 조용히 정상 결과로 만들지 않기 |
|    2 | fight.enemyNPCs와 필요시 enemyPets/enemyPlayers로 적 집합 구성 | 플레이어·우호 NPC 제외, helper 보존          |
|    3 | ability/actor lookup 사전 생성, timestamp 안정 정렬            | 반복 조회 절감과 SPEC 순서 보장              |
|    4 | 적 cast 선택, 일반공격과 catalog의 ignored 능력 제외           | 최소한의 유용한 초안                         |
|    5 | 완전히 같은 시전 키의 중복만 제거                              | 다른 instance나 실제 연속타를 지우지 않기    |
|    6 | 각 cast를 event + Ability sync로 변환                          | 관측한 실제 완료 시각 사용                   |
|    7 | sync 충돌 후보에 enabled:false와 note 표시                     | 반복 스킬의 잘못된 재동기화 방지             |
|    8 | provenance note와 abilityCatalog 생성                          | 초안 근거와 다음 생성의 제외 규칙 기록       |
|    9 | SPEC 검증을 통과한 뒤 새 파일로 저장                           | 기존 작성자의 YAML을 덮어쓰지 않기           |

다른 instance가 같은 ability를 같은 시각에 사용한 것은 기믹 한 번일 수도, 별개 대상의 실행일 수도 있다. 첫 버전은 그대로 남기고 검토 후보로 표시한다. 출력 개수를 줄이기 위해 임의의 1초 버킷으로 병합하면 실제 연속 공격까지 손실할 수 있다. 동시 helper를 한 행으로 표현하는 규칙은 encounter별 검토 이후 추가한다.

sync 충돌은 **출력 행뿐 아니라 제외된 원본 cast까지** 확인해야 한다. 같은 조건의 다른 cast가 해당 행의 window 안에 들어오면 동기화가 엉킬 수 있다. 기본 window는 앞뒤 2.5초이며, 두 출력 행의 window가 겹치는 경우도 검토 대상으로 삼는다. window가 길거나 서로 겹친다는 사실만으로 충돌은 아니다. 같은 조건에 다른 회차의 신호가 들어오는지를 확인하고, 충돌이 있을 때 조건 강화·window 축소·대표 행만 활성화 중 선택한다. `enabled:false`는 SPEC에서 의도적 sync 비활성화이므로 note에 검토 이유를 남긴다. [로컬 SPEC](../SPEC.md)

### 다중 로그의 내부 모델과 정렬

|           단계 | 권장 처리                                                                                           | 실패 방지                                                          |
| -------------: | --------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------ |
|   1. 입력 그룹 | encounterID·difficulty와 실제 능력/actor 호환성으로 자동 묶음                                       | 사용자가 ID를 입력하지 않음. 서로 다른 버전의 변형은 검출하고 보고 |
| 2. 역할 정규화 | report sourceID → actor.gameID·역할, ability 정수 ID, 종류로 토큰화                                 | 보고서 지역 ID·다국어 이름으로 병합하지 않음                       |
| 3. 이벤트 단위 | 시작과 완료는 대응시키되 별도 시각 유지. helper의 동일 시각 실행은 instance/개수 포함 묶음으로 보존 | 단순 dedup으로 실행 횟수나 공격을 삭제하지 않음                    |
|   4. 부분 순서 | 완전히 같은 timestamp에는 순서 대신 묶음 표현. 약간 차이나는 동시성은 packet·역할·간격으로 검토     | A→B/B→A를 무조건 병렬로 일반화하지 않음                            |
|        5. 앵커 | 공통이고 주변 맥락이 유일한 짧은 능력 묶음, phaseTransitions·보스 교체를 경계 후보로 활용           | 반복된 ability 하나로 페이즈 고정 금지                             |
|   6. 구간 정렬 | 앵커 사이에서 삽입·삭제·대안 허용 DP. 명확한 공통 prefix/suffix를 공유                              | gap을 missing event, 선택 경로, 종료 잘림으로 구별                 |
| 7. 경로 그래프 | 노드는 ability ID가 아니라 위치/맥락을 가진 시전 슬롯. 각 edge에 원본 로그·occurrence 근거 저장     | 모든 B528을 하나의 상태로 합쳐 가짜 cycle을 만들지 않음            |
|   8. 대안 병합 | 선택 슬롯을 조합하고 공통 suffix 공유. 관측 의존성이 있으면 필요한 이력 유지                        | 미관측 조합 허용은 기본이며 의존성 위반·시간 추정 오류를 검사      |
|   9. 반복 탐색 | 인접한 반복 블록, 여러 로그의 반복 횟수 차이, 내부 간격·출구를 함께 비교                            | 같은 이름/ID 반복만으로 loop 확정 금지                             |
|  10. 시간 추정 | 각 블록 진입 anchor 기준 offset의 중앙값과 min/max, 표본수 기록                                     | 전체 pull 시각 평균·평균 간격 누적으로 누적 오차 생성 금지         |
|     11. 컴파일 | 직선·대안 슬롯·분기 블록·검증된 loop를 SPEC entries로 변환                                          | 내부 그래프를 YAML 모델에 임의 필드로 끼우지 않음                  |
|       12. 검증 | 원본 전체 및 한 로그 제외 재생, 오매칭·누락·신규 경로·window 충돌 보고                              | 전환 시각 변동은 window로 수용하되 오매칭·누락을 숨기지 않음       |

앵커 구간 정렬은 sequence_align 등 기존 OSS, 발견·적합성은 PM4Py 등으로 검증하는 구성을 우선한다. 라이선스와 입력 제약은 [OSS 후보](#오픈소스-후보와-입력-제약)에 정리했다. POA 연구는 선형 합의로 줄이지 않고 정렬 그래프에 대안을 보존하는 접근의 근거이며, DAG 정렬에서 반복 압축은 별도 문제다. 이 적용은 생물학적 시퀀스 정렬을 이벤트 로그에 옮긴 설계 제안이다. [Lee et al. 2002](https://academic.oup.com/bioinformatics/article/18/3/452/236691), [Lee 2003](https://academic.oup.com/bioinformatics/article/19/8/999/235258)

프로세스 마이닝은 sequence·choice·parallel·loop 발견을 다루지만 불완전 로그와 잡음이 원본 모델 재발견을 제한한다. 일반 발견 알고리즘의 모델을 바로 타임라인으로 출력하면 sync로 관측 가능한지와 시간 의미를 놓친다. [Leemans et al., Scalable process discovery and conformance checking](https://pmc.ncbi.nlm.nih.gov/articles/PMC5910523/)

### 분기 조합 정책

타임라인은 앞으로의 기믹을 안내하므로 **알려진 선택 슬롯의 미관측 조합도 기본 허용**한다. 모델이 허용하는 조합과 게임에서 확인된 조합은 다음 상태로 구분한다.

| 상태                | 근거                                            | 생성 정책                                                                                 |
| ------------------- | ----------------------------------------------- | ----------------------------------------------------------------------------------------- |
| observed            | 해당 전체 경로를 정상 도달한 로그에서 실제 관측 | 지원 경로로 출력                                                                          |
| confirmed-allowed   | 버전이 맞는 외부 전투 규칙 또는 확인한 메커니즘 | 미관측이어도 해당 규칙 범위의 조합 지원 가능                                              |
| confirmed-forbidden | 직접적인 게임 규칙/상태 조건 근거               | 해당 조합 생성 제외. 빈도 0은 이 근거가 아님                                              |
| unknown             | 모델만 허용하거나 표본에서 아직 미관측          | 기본적으로 조합을 허용. 추론한 조합·시간 추정·표본수를 기록하고 실제 분기 신호로 재동기화 |

allowed/forbidden/unknown은 생성 분석 결과 또는 별도 encounter 규칙 파일의 개념이다. SPEC v1에 임의 필드를 추가하지 않는다. 규칙을 설정할 때 숫자 encounter ID 대신 전투명과 사람이 읽는 branch 이름을 사용하고, 내부에서 버전·능력 ID에 연결한다.

생성기는 **일반화한 모델을 기본 출력하고 관측 variants를 검증 근거로 보관**한다. 각 선택 슬롯의 알려진 대안을 조합하며, 선택 전에는 A/B 예고를 표시하고 선택 신호 이후에는 해당 블록에 sync한다. 미관측 전체 조합도 알려진 블록의 상대 시각을 재사용해 예측할 수 있지만, 그 시각은 실측이 아니라 조합에 의한 추정으로 기록한다. 필요한 블록이나 분기 판별 신호가 전혀 없으면 지원 한계를 보고한다.

분기 뒤 공통 앵커로 복귀하고 후속 순서·상대 간격이 같으면 선택 슬롯을 각각 구성한다. 이전 선택에 따라 후속 순서·간격·분기 신호가 달라지면 필요한 상태를 유지하거나 블록을 분리한다. 단순 빈도 차이·조합 부재나 독립성 검정의 비기각은 제약/독립성의 증명이 아니다.

| 일반화의 대상                     | 기본 처리                                                 | 검증                                 |
| --------------------------------- | --------------------------------------------------------- | ------------------------------------ |
| 알려진 대안의 미관측 조합         | 조합 허용, 공유 suffix와 블록 재사용                      | 선택 신호 이후 올바른 블록 진입      |
| 분기 이전의 미래 표시             | A/B 예고, 확정 경로처럼 표시하지 않음                     | 선택 전부터 유효한 안내인지          |
| 미관측 조합의 시간표              | 블록 진입 기준 상대시각으로 추정, 실제 sync로 보정        | 선택 이후 오차·누적 drift            |
| 관측된 과거 선택 의존성           | 최소한의 이력 유지, 필요한 구간만 분리                    | 관련 선택에 따라 후속 순서·간격 재현 |
| 알려지지 않은 기믹·종료·반복 횟수 | 조합 일반화와 별도. 새 능력이나 무한 반복을 창작하지 않음 | 추가 표본으로 구조 갱신              |

PM4Py의 parallel/optional 구조가 허용하는 생략·임의 순서까지 자동 수용한다는 뜻은 아니다. 일반화는 정렬된 선택 슬롯의 알려진 대안 조합에 우선 적용한다. 검사에서 새 조합이 나왔다는 사실은 오류가 아니며, 오작동·관측된 의존성 위반·잘못된 시각 예측을 오류로 다룬다.

부분 표본으로 전체 언어를 확정하는 문제 자체가 process mining의 과적합·과소적합 균형 문제다. [van der Aalst et al. 2010](https://research.tue.nl/en/publications/process-mining-a-two-step-approach-to-balance-between-underfittin/)

### SPEC 필드 대응

| SPEC 필드                                     | 입력 / 변환                                           | 기본 정책                                                       |
| --------------------------------------------- | ----------------------------------------------------- | --------------------------------------------------------------- |
| schemaVersion                                 | 고정 `1`                                              | 항상 출력                                                       |
| event.at                                      | 단일 pull 상대시각 또는 블록 offset + 가상 진입 시각  | 원본 밀리초로 계산 후 마지막에 0.1초 반올림                     |
| event.name                                    | masterData.abilities의 gameID = event.abilityGameID   | 기준 로그의 이름; 의미 불명은 note로 검토 표시                  |
| sync.log                                      | cast → `Ability`, begincast → `StartsUsing`           | 기본은 cast/Ability                                             |
| sync.fields.id                                | 정수 abilityGameID를 대문자 16진 문자열로 변환        | 예: 46376 → `"B528"`; actor/status ID와 혼동하지 않음           |
| sync.fields.source                            | sourceID에 대응하는 actor.name                        | 정규식 특수문자를 escape. 이름이 불명확하거나 충돌하면 검토     |
| event.duration                                | 선택한 시작 이벤트에서 실제 종료까지                  | cast 완료 행에는 begincast.duration을 자동 복사하지 않음        |
| abilityCatalog                                | 적 cast 및 begincast에서 관측한 능력                  | 첫 관측 순서로 ID 중복 제거; phase catalog는 별도 유지          |
| abilityCatalog.ignored                        | 작성자의 제외 결정                                    | 일반공격은 우선 제외 후보, 기믹 중요도는 이름으로 추정하지 않음 |
| generatorOptions.targetable                   | 선택한 보스의 targetabilityupdate                     | 사용자가 대상 지정한 경우만 메타데이터에 반영                   |
| generatorOptions.ignoredCombatants            | 작성자의 이름 기반 제외 목록                          | 기존 SPEC가 이름을 사용하므로 내부 actor 구분과 차이를 주의     |
| generatorOptions.phaseStarts                  | 페이즈 기준 ability와 가상 타임라인 시각              | phaseTransitions를 그대로 복사할 수 없음                        |
| label / jump / window / hideNames / syncOrder | 다중 로그 경로 분석과 sync 검증 결과                  | 반복·분기를 보존하도록 자동 컴파일; 근거 부족 구간은 별도 보고  |
| note                                          | 보고서 코드·fight ID·revision·logVersion·시간 기준 등 | SPEC에 새 최상위 필드를 만들지 않고 기존 note 사용              |

### SPEC v1로 표현하는 방법

| 구조                            | 표현                                                       | 자동 출력 조건                                                            |
| ------------------------------- | ---------------------------------------------------------- | ------------------------------------------------------------------------- |
| 공통 직선 구간                  | 일반 event, 대표 offset, enabled sync                      | 대응과 시간 변동 검증                                                     |
| 같은 시각·같은 이후 흐름의 대안 | name을 A/B로 표시, fields.id 배열                          | 후속 경로·시간이 실제로 같음을 확인. 기믹 의미가 동일하다고 주장하지 않음 |
| 다른 후속 경로의 선택           | 선택 신호에 when:sync jump, 분기별 label과 가상 시각 구간  | 구별 가능한 런타임 신호 존재                                              |
| 공통 합류                       | branch 끝에 sync jump로 공통 label 이동                    | 합류 신호의 ID·맥락이 검증됨                                              |
| 검증된 항상 반복                | 반복 시작 label, 반복 종료 sync + when:always              | 스케줄과 외부 탈출 sync를 확인한 경우만                                   |
| 조건부 반복 / HP 전환           | when:sync, exit sync 우선순위·충돌 확인, lookahead 행 펼침 | 반복/전환을 구별하는 관측 신호 있음                                       |
| 짧은 관측 반복, 근거 부족       | 반복 행을 유한하게 펼쳐 보존, note로 한계 표시             | 무한 반복을 추정하지 않음                                                 |
| 분기 식별 신호 없음             | 공통 구간과 A/B 표시, 별도 보고                            | 정확한 분기별 미래 표시 불가                                              |

SPEC에는 graph/branch entry가 없다. 내부 분석 그래프를 기존 label·event·jump로 컴파일해야 한다. label은 실행 범위를 제한하는 블록이 아니라 가상 시각의 이름이고, sync는 현재 시간의 window에서 활성화된다. branch별 가상 구간을 분리하고, 다른 branch sync가 활성화되지 않는지 검사해야 한다. [SPEC](../SPEC.md)

분기가 길면 가상 시각을 겹치지 않는 구간에 배치한다. 진입 sync 시각에서 branch label로 jump하고, branch 종료에서 공통 suffix label로 jump한다. 구간 간격은 고정 1000초 같은 추측보다 블록 최대 길이·window·lookahead를 포함해 계산한다. entries는 최종 가상 시각으로 안정 정렬하면 SPEC의 시간 순서 검사를 지킬 수 있다. syncOrder 비활성화를 일반 해결책으로 쓰지 않는다.

### 긴 window를 이용한 페이즈 전환 생성

긴 window는 페이즈 구분과 전환을 구현하는 유효한 수단이다. 전환 시각이 HP나 진행에 따라 달라지면, 전환 이벤트의 sync를 긴 window 안에서 활성화해 신호를 기다린다. 매칭되면 `when: sync` jump로 다음 페이즈 label의 가상 시각에 진입하고, 이후 이벤트는 해당 페이즈 진입 기준 상대시간으로 배치한다. window와 페이즈별 시간 모델은 함께 사용한다. [SPEC의 Sync 및 Jumps](../SPEC.md)

| 생성 단계      | 처리                                                                        | 검증                                                            |
| -------------- | --------------------------------------------------------------------------- | --------------------------------------------------------------- |
| 전환 신호 선정 | 실제 관측 가능한 능력 ID와 source 등으로 sync 구성                          | 같은 조건이 앞선 회차나 다른 분기에서도 나타나는지 확인         |
| 활성 범위 산정 | 이전 sync/jump로 보정된 타임라인 시계에서 전환의 이른/늦은 도달 범위를 계산 | pull 기준 timestamp 범위와 가상 시계의 범위를 혼동하지 않음     |
| window 생성    | 기준 at에 대해 필요한 before/after를 각각 산정. 비대칭·긴 window 허용       | 관측 범위와 추가 여유를 구분하고, 학습 표본 밖 도달은 별도 보고 |
| 페이즈 진입    | 전환 sync에 `jump: { to: phase2, when: sync }` 부여                         | 전환을 못 봤을 때 임의로 넘어가지 않도록 forcejump와 구분       |
| 후속 배치      | phase2 label 기준으로 이후 행의 상대시간 배치                               | 진입 뒤의 sync/window와 다른 가상 구간의 활성 조건 검사         |

아래는 긴 window와 jump의 **형식 예시**다. ID와 시각은 설명용 가상 값이며 현재 로그에서 추출한 결과가 아니다.

```yaml
schemaVersion: 1
hideNames: ["--sync--"]
entries:
  - kind: event
    at: 300.0
    name: "--sync--"
    sync:
      log: StartsUsing
      fields: { id: "ABCD", source: "Example Boss" }
      window: [100, 200]
    jump: { to: phase2, when: sync }
  - kind: label
    at: 1000.0
    name: phase2
  - kind: event
    at: 1010.0
    name: "Phase 2 mechanic"
```

이 예시의 전환 sync는 타임라인 시계 200–500초 범위에서 신호를 감지하고 1000초로 jump한다. 후속 행은 진입 10초 뒤에 배치된다. label 자체는 페이즈를 격리하지 않으므로 다른 가상 구간의 sync가 동시에 잘못 활성화되지 않는지도 검사해야 한다. 페이즈 전환을 위한 넓은 감지 범위와 반복 토큰 오정렬을 감추기 위한 window 확대는 목적이 다르다. **안전성 판단 기준은 window 길이 자체가 아니라 활성 범위 안에서 의도한 전환 신호를 구별할 수 있는가**다.

Dancing Mad의 14.689초 관측 변동은 전환 sync의 window가 수용해야 할 관측 근거다. Clyteum의 수백 초 차이는 정렬 오류와 실제 진행 차이가 섞일 수 있으므로 그 값을 바로 window로 복사하지 않는다. 두 경우 모두 전환 맥락을 먼저 정렬한 뒤 가상 시계에서 필요한 감지 범위를 구한다. 미관측 HP 전환 시각까지 보장하려면 추가 표본이나 별도 근거가 필요하다.

### SPEC 계약과 미검증 사항

SPEC 출력에는 event·label·jump 등 기존 필드를 사용한다. occurrence 근거·분기 관측 상태·시간 오차는 별도 보고서에 두고, graph/branch/confidence 같은 필드를 YAML에 임의 추가하지 않는다. [SPEC](../SPEC.md)

| 쟁점              | 현재 계약                                    | 조사 결과 / 선행 작업                                                                         |
| ----------------- | -------------------------------------------- | --------------------------------------------------------------------------------------------- |
| 출력 및 기본 검증 | JSON Schema→semantic→cactbot parser          | 현재 Rust CLI는 수집기이며 generator/schema/renderer는 아직 없다                              |
| 독립 검증 방침    | 기존 연구는 공식 cactbot 검증 코드 사용 제외 | SPEC parser gate와 연구 방침을 정리해야 함. 독립 재생만으로 parser gate 충족을 주장할 수 없음 |
| sync 선택         | window 기본 ±2.5초, jump 조건 규정           | 여러 sync가 같은 신호에 매칭될 때의 선택 순서와 경계 처리 미명시                              |
| 반복 탈출         | sync jump와 forcejump 지원                   | 같은 시각 exit sync/forcejump 우선순위 미명시. 검증 전 무조건 loop 생성 제한                  |
| 미리 보기         | SPEC에 lookahead 계약 없음                   | 분기 전 대안 예고는 가능하나 분기별 가상 구간의 실제 표시까지 검증됐다고 주장할 수 없음       |
| network-log 검증  | 알려진 로그 정의·필드·regex를 검사해야 함    | 독립 사용 가능한 정의와 regex 호환 범위가 필요. FFLogs 정수 필드를 이름만 바꿔 넣으면 부족    |
| 종료 잘림         | SPEC에는 censoring 필드 없음                 | 분석 내부에서 kill/wipe 종료를 처리하고 출력 note/보고서에 범위를 기록                        |

위 의미를 확정하기 전에도 직선·대안 슬롯 초안 실험은 가능하다. 그러나 label/jump가 있는 결과에 “공식 런타임 호환 검증 완료”를 붙여서는 안 된다. SPEC 변경과 generator 구현은 아직 수행하지 않았다.

## OSS 검토와 실험

### 오픈소스 후보와 입력 제약

| 후보                                                                    | 재사용 범위                                                                  | 라이선스 / 제약                                                         | 판단                                                                            |
| ----------------------------------------------------------------------- | ---------------------------------------------------------------------------- | ----------------------------------------------------------------------- | ------------------------------------------------------------------------------- |
| [sequence_align](https://github.com/kensho-technologies/sequence_align) | Needleman–Wunsch, Hirschberg, 사용자 점수 함수                               | Apache-2.0. 문자열 토큰 및 custom scoring API; Rust 코어·Python binding | **첫 정렬 후보**. ability ID를 DNA 문자로 바꿀 필요 없이 역할/종류/ID 토큰 사용 |
| [PM4Py](https://github.com/process-intelligence-solutions/pm4py)        | process discovery, trace variants, prefix tree, alignment, fitness/precision | AGPL-3.0, 별도 commercial license 안내. Python·수치 라이브러리 의존성   | **첫 모델 연구 후보**. 게임 규칙의 자동 확정 엔진은 아님                        |
| [Rust-Bio POA](https://docs.rs/bio/latest/bio/alignment/poa/index.html) | 다중 시퀀스의 DAG 정렬                                                       | MIT. API 입력은 u8 symbol; 구간별 토큰을 256개 이내로 대응시킬 필요     | Rust 안에 정렬을 넣을 때 대안. 반복·제약 추론은 별도                            |
| [SPOA](https://github.com/rvaser/spoa)                                  | SIMD POA, MSA/graph                                                          | MIT. C++ 빌드/연결 필요                                                 | 지금 데이터량에는 성능보다 도입 비용이 큼                                       |
| [abPOA](https://github.com/yangao07/abPOA)                              | adaptive banded POA, MSA/GFA                                                 | MIT. C/native build, 생물학 alphabet 제약 확인 필요                     | 우선순위 낮음                                                                   |
| [Rust4PM](https://github.com/aarkue/rust4pm)                            | XES/OCEL·Petri net, Rust alignment                                           | MIT 또는 Apache-2.0. patch에서도 API 변경 가능하다는 안내               | Rust conformance 대안. 이번 조사에서 PM4Py IM의 대체 발견 API는 확인 못 함      |

sequence_align은 pairwise 정렬이다. 모든 로그를 순차 병합할 때 순서에 따른 결과 변화를 검사해야 하며, graph 병합·문맥·분기 관계는 별도 연결 단계다. custom scoring으로 역할·능력·종류·블록 상대시각을 반영할 수 있지만, timing 가중치가 실제 HP 전환이나 대안을 억지로 같은 행에 맞추지 않는지 검증한다. [실제 API 소스](https://github.com/kensho-technologies/sequence_align/blob/main/src/sequence_align/pairwise.py)

Rust-Bio의 POA graph는 DAG다. 같은 loop의 회차는 우선 펼친 occurrence로 존재한다. 정렬 graph에서 공통 노드를 공유했다고 게임 상태가 동일하거나 두 선택이 독립이라는 뜻은 아니다. [Poa API](https://docs.rs/bio/latest/bio/alignment/poa/struct.Poa.html)

Rust4PM은 Rust 안에서 적합성 검증을 재사용할 때 검토한다. 현재 공개 문서에는 process model과 conformance가 있고, 실제 exact alignment는 trace 위치와 model transition을 연결한다. 도입 시 버전을 고정하고 탐색 제한에 도달한 결과를 불일치와 구별한다. [crate 문서](https://docs.rs/process_mining/latest/process_mining/), [alignment 소스](https://docs.rs/process_mining/latest/src/process_mining/conformance/case_centric/alignments/mod.rs.html)

PM4Py의 실제 통합은 AGPL 조건과 배포 방식의 적합성을 결정한 뒤 진행한다. 별도 프로세스 호출만으로 라이선스 문제가 해소된다고 가정하지 않는다. 현재는 연구 환경용이며 Cargo 의존성으로 추가하지 않았다.

`sequence_align`의 지원 기능은 기존 조사에서 확인했으나 실제 로그에 라이브러리를 실행한 실험은 수행하지 않았다. pairwise 정렬만으로 분기 독립성·반복·시간 모델이 해결되지는 않는다.

### PM4Py 합성 실험

임시 venv에 PM4Py **2.7.23.8**을 설치했다. 프로젝트 Cargo/Python 의존성은 바꾸지 않았다. 다음은 합성 시퀀스로 discovery와 exact alignment를 시험한 결과이며 실제 R12S 생성기의 검증 결과는 아니다.

| 관측 시퀀스의 중간 두 선택 | 발견 모델이 수용한 조합 | 의미                                                                          |
| -------------------------- | ----------------------- | ----------------------------------------------------------------------------- |
| AC, BC, BD                 | AC, AD, BC, BD          | 미관측 AD까지 일반화함. 실제 게임에서 AD 가능 여부는 이 모델로 확정할 수 없음 |
| AC, BD                     | AC, BD                  | 상관된 두 경로를 묶어 표현. 표본 부족한 독립 선택도 이렇게 보일 수 있음       |
| AC, AD, BC, BD             | 네 조합 모두            | 두 선택을 별도 XOR로 표현. 관측한 네 조합 수용은 확인 가능                    |

첫 입력에서 생성된 process tree에는 parallel/optional 구조도 포함된다. 조합만 맞아 보이는 것으로 충분하지 않고, 스킬 생략·추가 순서·동시성이 새로 허용되는지도 검사해야 한다. 발견 모델의 구조를 그대로 SPEC로 번역하지 않는다.

재현: 임시 Python 환경에 `pm4py==2.7.23.8`을 설치한 뒤 `python scripts/research_pm4py_branches.py`. [실험 스크립트](../scripts/research_pm4py_branches.py). alignment에서 무음 transition은 허용하고 **가시 이벤트의 log/model move가 없을 때** 수용으로 판정했다. 무음 transition도 기본 비용을 가질 수 있어 총 비용 0만 검사하는 방식은 사용하지 않았다.

| PM4Py 사용 규칙                                 | 이유                                                                            |
| ----------------------------------------------- | ------------------------------------------------------------------------------- |
| 최초 noise_threshold=0                          | 희귀한 실제 분기를 빈도 기준으로 삭제하지 않음                                  |
| trace 로그를 입력, DFG는 보조 시각화            | IMd/DFG는 멀리 떨어진 선택의 전체 경로 관계를 잃음                              |
| timestamp는 별도 timing 통계 유지               | miner가 activity 시퀀스로 투영하므로 cast 간격·window·조건은 자동 학습되지 않음 |
| prefix tree/관측 variants를 발견 모델 옆에 유지 | 원본이 실제 지원한 조합과 모델의 일반화를 비교                                  |
| alignment·fitness·precision을 분리해서 보고     | 높은 학습 fitness가 게임에서 미관측 경로까지 가능하다는 증거는 아님             |

이 선택과 API 동작은 [discovery 소스](https://github.com/process-intelligence-solutions/pm4py/blob/release/pm4py/discovery.py), [Inductive Miner 구현](https://github.com/process-intelligence-solutions/pm4py/blob/release/pm4py/algo/discovery/inductive/algorithm.py)을 확인했다. 작은 실험에 성공했다는 사실만으로 현재 원시 로그 전체의 모델 발견 품질을 보장하지 않는다.

## 구현 순서와 완료 기준

| 순서 | 최소 작업                                             | 완료를 판단할 근거                                                  |
| ---: | ----------------------------------------------------- | ------------------------------------------------------------------- |
|    1 | 현재 224개 입력 감사와 provenance 보존                | 통합 감사 스크립트의 출력. 향후 입력에도 그룹·완료·참조 검사 적용   |
|    2 | 그룹별 enemy boss+helper 시작/완료, 동시묶음 정규화   | 원본 occurrence로 역추적 가능; helper 실행 횟수 보존                |
|    3 | 104/105 구간 정렬·체크포인트 진입 연결·후반 선택 비교 | 104 시작과 105 재시작 모두 지원; 네 조합·후속 의존성·종료 잘림 보존 |
|    4 | SPEC 모델/검증/직렬화 및 첫 YAML 출력                 | 0.1초 최종 반올림, regex escape, catalog·note 보존, 결정적 출력     |
|    5 | 관측 신호 재생과 별도 report 검증                     | 활성 sync 오매칭·누락·시간 오차·분기 진입/합류를 분리 보고          |
|    6 | Dancing Mad phase 경계 및 Clyteum 반복 확장           | 실제 진입/출구 신호와 반복 회차 검증 후 label/jump 압축             |

첫 R12S 출력은 정렬된 대안 슬롯부터 만든다. PM4Py는 합성 실험의 모델 후보·적합성 비교 도구로 유지하고, 모델이 복잡해질 때 통합을 결정한다. generic DP/miner는 재구현하지 않는다.

추가 표본의 우선순위는 **R12S 104 종료→105 진입의 구간 밖 이벤트와 체크포인트 재진입 신호 → 후반 B537의 종료 조건·sync 충돌 및 미관측 후속 여부 검증 → Clyteum 전환 직전/후반 wipe**다. Dancing Mad 후반 wipe는 8개 확보했으므로 먼저 해당 신호를 분석한다. 파일 수만 늘리기보다 종료·반복 출구·구간 coverage를 늘리는 것이 지금의 미검증 사항을 줄인다.

이 문서는 로컬 224개 로그와 기존 감사 코드의 관측을 정리했다. 외부 API·라이브러리를 새로 검증하지 않았으며, 외부 링크는 이전 조사에서 보존한 근거다.
