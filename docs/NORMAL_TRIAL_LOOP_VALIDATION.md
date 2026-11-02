# Doomtrain·Enuo 일반 난이도 반복 검증

2026-11-02 기준, 두 일반 토벌전을 검증 코퍼스에 추가했다. **생성본의 구조 검사는 통과하지만 두 전투 모두 자동 생성 loop는 0개다.** 참조 반복이 원본에 존재하는지와 최종 KDL이 이를 예측하는지는 별도로 판정한다.

## 난이도와 입력 고정

| 전투      | 일반 encounter / game zone | 극 encounter / game zone | 선택 입력       | 전투 길이     |
| --------- | -------------------------- | ------------------------ | --------------- | ------------- |
| Doomtrain | 1547 / 1307                | 1083 / 1308              | 처치 5 + 전멸 1 | 449.9–670.3초 |
| Enuo      | 1548 / 1361                | 1084 / 1362              | 처치 5 + 전멸 1 | 455.7–600.5초 |

FFLogs의 극 토벌전 zone도 difficulty 100을 `Normal`로 보고한다. **difficulty=100만으로 일반 난이도를 판별하면 안 된다.** 검증 스크립트는 encounter·difficulty·game zone 세 값을 모두 검사하며 누락된 zone도 거부한다. 이 ID들은 검증 입력의 식별자이며 생성기의 전투별 규칙이 아니다.

일반 encounter는 당시 worldData encounter 조회에서 나오지 않아, `reportData.reports(gameZoneID: …)`의 공개 보고서 목록에서 code를 확보한 뒤 각 보고서를 직접 조회했다. Doomtrain은 목록 page 1·10, Enuo는 page 1·10·25에서 조사했다. 긴 처치와 짧은 처치·전멸을 포함하도록 각 6개를 선택하고, 선택한 보고서에 일반 encounter·difficulty 필터로 수집 계획을 만들었다. 다운로드 후 12개 모두 완전 수집·단일 fight·일반 zone임을 재검사했다. 입력 경로·trace·SHA-256·실측 길이는 [입력 명세](normal-trial-inputs.csv)에 고정했다. 원본 로그와 API 응답은 로컬 `logs/normal-*`, `out/validation`에 보관한다.

## cactbot 기준과 원본 관측

참조는 cactbot 커밋 `e79ea7f16093be5b80cda0e2d5bc8eb899d6fb4b`의 [doomtrain.txt](https://github.com/OverlayPlugin/cactbot/blob/e79ea7f16093be5b80cda0e2d5bc8eb899d6fb4b/ui/raidboss/data/07-dt/trial/doomtrain.txt), [enuo.txt](https://github.com/OverlayPlugin/cactbot/blob/e79ea7f16093be5b80cda0e2d5bc8eb899d6fb4b/ui/raidboss/data/07-dt/trial/enuo.txt)다. 공식 parser로 label과 뒤로 가는 forcejump를 읽으며, 참조 파일은 생성기에 입력하지 않는다.

| 구간            | 참조 시작 → 뒤로 이동하는 경계 | 명목 주기 | 원본 검사 결과                                                                      |
| --------------- | ------------------------------ | --------: | ----------------------------------------------------------------------------------- |
| Doomtrain car 5 | 603.5 → 709.4                  |   105.9초 | 처치 5개에서 전체 순서 창 6개. 활성 본문 14단계와 다음 StartsUsing B9A7 경계를 대응 |
| Enuo adds       | 185.2 → 200.2                  |    15.0초 | 같은 C326 연속 창 73개. beacon과 같은 창을 가리키므로 독립 회차 수로 해석 불가      |
| Enuo beacon     | 306.0 → 321.0                  |    15.0초 | adds와 구분 불가. 5개 본문은 commented Ability 주석이며 실제 sync로 취급하지 않음   |
| Enuo P2         | 575.3 → 784.5                  |   209.2초 | 활성 본문 41개 패턴이 각 입력에 있으나 엄격한 전체 순서 창은 0개. 대응 검증 미완료  |

Doomtrain은 첫 본문 신호가 label보다 1.4초 늦다. 첫 신호부터 다음 경계까지의 실측은 104.653–104.746초이며, 참조 첫 offset을 더한 진단값은 106.053–106.146초다. 이 값은 실제 loop 시작을 관측했다는 뜻이 아니다. Hail Thunder의 참조 주석에도 약 3초 변동이 있으며, 시각 차이 자체로 구조 대응을 거부하지 않는다. 본문은 Doomtrain·Levin Signal·Kinematic Turret 등 여러 source를 포함한다. 출력에 대응된 원본 멤버는 창마다 18개 중 5–7개로, 유한 생성도 전체 본문을 복원하지 못한다.

Enuo의 C326만으로는 adds와 beacon을 구분하거나 15초 회전을 유일하게 정할 수 없다. 같은 관측 창을 두 번 세지 않으며, 최소 반복 단위와 전투 문맥을 별도로 학습해야 한다. P2에는 ID 대안의 동시 파동, 추가 cast 시작, 주석 처리된 표시, 사망으로 취소될 수 있는 Naught Hunts가 있다. 현재 엄격한 대응 실패를 반복 부재나 참조 오류로 해석하지 않는다. 가장 긴 기록에는 후반 반복 경계 후보도 있어 단순히 짧은 로그 탓으로 결론 내릴 수 없다.

| 참조 제어 신호                                              | 현재 해석                                                     |
| ----------------------------------------------------------- | ------------------------------------------------------------- |
| Doomtrain NameToggle, Aether 261 → intermission-enrage 2000 | 앞으로 이동하는 실패 경로이며 loop로 세지 않음                |
| Enuo MapEffect, beacon·P2 전환                              | FFLogs 시전만으로 실제 런타임 신호를 복원했다고 주장하지 않음 |
| Enuo StartsUsing C327 대체 경계                             | cast 완료와 구분한 관측 타입으로 검사                         |

## 도구와 생성 결과

`scripts/audit_normal_trials.ts`는 원본 cast·begincast를 구분하고 ID·영어 source·순서로 검사한다. 서로 다른 actor/instance의 같은 ID가 100ms 안에 발생하면 한 파동으로 대응하되 모든 원본 인덱스를 보존한다. 같은 개체의 재시전은 이 규칙으로 합치지 않는다. 대칭적인 본문은 `ambiguous_rotation`으로 표시한다. KDL과 입력 SHA-256도 검사한다. 이는 ACT 재생이나 최종 표시 시각·미래 회차 예측 검사가 아니다.

| 생성 파일                  | 이벤트 정의 | loop | 구조 검사 |
| -------------------------- | ----------: | ---: | --------- |
| `out/doomtrain-normal.kdl` |         120 |    0 | 통과      |
| `out/enuo-normal.kdl`      |          94 |    0 | 통과      |

최초 조사에서는 raid 모드가 자동 반복 발견을 적용하지 않았다. 이후 모든 mode에서 같은 말단 반복 학습을 실행하도록 수정했다. raid는 전투 전체의 실행 적격 적대 시전을 함께 학습하며 실제 source별로 동기화한다. 두 실제 토벌전은 여전히 0 loop이고 KDL 해시는 최초 생성본과 같다. Doomtrain에는 도입부·본문 순서 차이와 패킷 분절, Enuo에는 패킷 분절·고유 경계 부족·말단 본문 차이의 거부 사유가 남는다. 이 변경은 분석을 가능하게 한 기반이며 두 반복을 학습한 결과가 아니다. `trial` 모드는 추가하지 않았다.

검증을 통과시키기 위해 dungeon 모드로 바꾸거나 cactbot ID·주기를 생성기에 주입하지 않았다. Enuo 입력에서 enemyNPCs에 등록된 이름 없는 NPC 때문에 생성이 중단되는 문제도 발견했다. 공통 변환에서 이름을 null로 보존하고 실행 학습에서는 제외하도록 수정했으며, listed/owned 조합과 tether 끝점 회귀 검사를 추가했다.

| 공통 반복 분석의 회귀 검사             | 결과                                                                                                                                              |
| -------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------- |
| NPC 3개·Boss 메타데이터 없는 합성 전투 | raid의 InCombat 시작과 전투 상대 loop를 생성. evidence 삭제 뒤 KDL만으로 관측 없는 미래 회차 표시                                                 |
| 합성 보고서 1개 보류                   | 학습 2개·별도 관측 1개, 보류 기록은 시전 간격 10% 증가. 3개 기록 모두 원본 멤버의 표시 예측 존재, 최근접 같은 신호 표시와 최대 차이 300ms, 진단 0 |
| 같은 ID의 서로 다른 시전자             | loop event ID를 source로 구분하고 sync의 source·ID 대응 보존                                                                                      |
| 내부 단계가 빠진 입력                  | loop를 만들지 않고 유한 출력·미해결 진단 유지                                                                                                     |
| 기존 7개 + 두 일반 토벌전              | 9개 KDL 해시 유지. R11S 반복 탐색은 처리량 상한도 기록하며 미해결 상태 유지                                                                       |
| 기존 본체별 loop 재생                  | Griffin 12개 범위·Chirurgeon 15개 범위에서 예측 누락·5초 초과·진단 0. 최대 차이 각각 994ms·3,019ms                                                |

재생 보조 스크립트 `scripts/replay_dungeon_loops.ts`는 raid도 지원한다. raid 시작은 합성 InCombat이며 ACT 원본이 아니다. 회귀 자료는 `out/validation/combat-loops/`에 보관했다. 최근접 표시 대조를 일대일 경로 검증으로 해석하지 않는다.

이후 국소 진입 문맥 개선에서는 반복 앞의 전체 opener 일치 요구를 줄였다. 합성 raid·dungeon의 서로 다른 튜토리얼 길이에서 학습하며, 문맥 신호가 유한 sync로 출력되지 않으면 loop를 보류한다. Clyteum Malphas에 실제 반복이 추가됐지만 두 일반 토벌전은 아직 0loop이고 KDL 해시도 유지된다. 최신 회귀 자료는 `out/validation/context-guard/`와 [Clyteum 감사](CLYTEUM_OMISSION_AUDIT.md)에 있다. 이 개선만으로 Doomtrain·Enuo의 파동·본문 대응 문제가 해결되지는 않았다.

생성 및 관측 감사를 재현하려면 저장소 루트에서 다음을 실행한다. 출력 경로가 이미 있으면 다른 경로를 사용한다.

```sh
target/debug/btimeline generate logs/normal-doomtrain/*.json --mode raid -o out/doomtrain-normal.kdl
target/debug/btimeline generate logs/normal-enuo/*.json --mode raid -o out/enuo-normal.kdl
target/debug/btimeline validate out/doomtrain-normal.kdl
target/debug/btimeline validate out/enuo-normal.kdl
bun scripts/audit_normal_trials.ts doomtrain out/doomtrain-normal.evidence.json ../cactbot -o out/validation/doomtrain-normal.audit.json
bun scripts/audit_normal_trials.ts enuo out/enuo-normal.evidence.json ../cactbot -o out/validation/enuo-normal.audit.json
cd scripts && bun run test
```

## 다음 구현의 통과 조건

| 남은 작업                       | 검증 조건                                                                                                |
| ------------------------------- | -------------------------------------------------------------------------------------------------------- |
| 실제 일반 토벌전 반복 학습      | 공통 반복 발견은 raid에도 적용했다. Doomtrain 본문·다음 경계를 학습하고 미래 회차를 표시하는 검증은 남음 |
| helper·동시 파동·부분 관측 대응 | 원본 멤버의 누락과 순서 불확실성을 분리하고 Enuo P2 전체 대응 실패를 해소                                |
| adds·beacon 반복과 탈출         | 같은 ID 회전을 문맥과 전환 신호로 구분하고 종료를 영구 반복으로 오인하지 않음                            |
| 독립 검증                       | 보고서 전체 보류, 짧은 처치·전멸, 경계 누락과 다음 회차 사전 표시를 검사                                 |

이 코퍼스 추가는 반복 자동 생성의 완료가 아니다. 실제 ACT와 외부 실행기는 검사하지 않았다.
