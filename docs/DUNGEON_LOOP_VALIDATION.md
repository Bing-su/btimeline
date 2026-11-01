# 일반 던전 반복 검증

**Chirurgeon General과 Thundergust Griffin의 본체 이름에 속하는 시전 반복을 자동 생성한다.** Griffin은 이름이 다른 helper가 빠져 전체 기믹 검증이 미완료이며, 나머지 7개 참조 보스의 반복도 생성하지 못한다. 유한 로그의 공통 시전 복원과 반복 모델링은 구분한다. cactbot에 타임라인이 없는 Clyteum 대신 Alexandria·Mistwake·Meso Terminal을 검사했다. 비교 기준은 고정 커밋과 [9개 보스 반복 조사](CACTBOT_DUNGEON_LOOP_REFERENCE.md)에 기록했다.

## 조사 시작 시점의 입력과 생성 결과

| 던전               | 기존 입력 / 추가 입력           | 명시적 참조 반복 | 생성 정의 / loop |
| ------------------ | ------------------------------- | ---------------: | ---------------- |
| Alexandria         | `logs/loop-alexandria` 5개      |                3 | 49 / **0**       |
| Mistwake           | `logs/loop-mistwake` 8개        |                3 | 87 / **0**       |
| Meso Terminal      | `logs/loop-meso-terminal` 14개  |                3 | 48 / **0**       |
| Meso Terminal 확장 | 위 14개 + `3Cy9dQN21DKYLABg/14` |                3 | 37 / **0**       |

조사 당시 28개 파일을 사용했다. 아래 Mistwake 추가 4개를 포함해 총 32개 파일의 경로·trace·해시를 [입력 명세](dungeon-loop-inputs.csv)에 고정했다. Meso 추가 로그는 encounter 4547, difficulty 10, rankings page 30의 보고서에서 확보했다. 1,051.881초의 던전 기록이며 이벤트 23,362개다. 던전 전체 길이가 개별 보스의 여러 회차를 보장하지는 않는다. 추가 입력 후 정의 수 감소도 반복 학습의 성공으로 해석하지 않는다.

## 원본에 실제 반복 본문이 있는가

공식 cactbot parser로 label과 뒤로 가는 forcejump를 읽는다. 활성 Ability의 ID·source·순서를 검사하며 OR ID는 보존한다. 참조가 OR로 정의한 다른 ID가 100ms 이내 같은 파동에 나오면 원본 노드를 보존한 채 한 단계로 비교한다. commented Ability와 StartsUsing은 이 관측 검사에 넣지 않는다. 경계 ID가 여러 번 나왔다는 사실만으로 회차를 세지 않는다.

처치된 시도뿐 아니라 본체가 관측된 미완료 개체 범위도 **각각 분리해서** 검사한다. 미완료 범위의 마지막 본체 관측은 실제 종료 신호로 해석하지 않는다. 서로 다른 개체 범위를 연결해 가짜 회차를 만들지 않는다.

Meso Terminal의 Chirurgeon General에서 활성 본문 7단계와 다음 경계가 일치했다. 각 본문의 원본 파동 후보는 대안 ID 파동을 포함해 8개다.

| trace                  | 관측 경계 간격 | 참조 51.5초 대비 본문·경계 최대 offset 차이 | 출력 시각 표본으로 대응된 원본 후보 |
| ---------------------- | -------------: | ------------------------------------------: | ----------------------------------: |
| `7xf2dqWzVNFrLCJn/3`   |       51.548초 |                                     0.100초 |                               4 / 8 |
| `GrByCA3mZvDa2jLM/131` |       52.153초 |                                     0.653초 |                               4 / 8 |
| `3Cy9dQN21DKYLABg/14`  |       51.529초 |                                     0.102초 |                               4 / 8 |

세 자료에서 반복 전체 문맥이 관측돼도 생성본은 각 본문의 절반만 시각 표본에 대응시키고 loop는 만들지 않았다. 단순한 로그 부족만으로 설명할 수 없다. 특히 두 기록은 완료 학습 범위 밖의 개체에서도 본문이 관측되므로, 완료 모델의 적합성만으로 실패 시도의 구조 검증을 대신할 수 없다.

나머지 8개 보스는 이 검사에서 정확한 전체 본문과 다음 경계의 대응을 확보하지 못했다. 짧은 처치, 일부 reference 단계의 비관측, 순서 불확실성·대안·helper·여러 본체의 범위 문제가 포함될 수 있다. **반복이 없거나 cactbot이 틀렸다는 판정이 아니다.** 그 보스들의 무한 반복 검증은 미완료다.

## 실행기는 무한 반복을 실행하는가

생성 결과와 별도로, cactbot의 Chirurgeon General 본문을 기존 KDL 문법으로 옮긴 [실행기 fixture](../tests/fixtures/timeline/meso-terminal-loop.kdl)를 검사했다. 자동 생성본으로 취급하지 않는다. 관측 fixture는 `GrByCA3mZvDa2jLM/131`의 198,886–282,278ms에서 추출한 시전 15개를 Ability로 투영한 자료다. 초기 진입과 이후 일부 회차를 포함하며 실제 ACT 기록은 아니다.

| 실행 검사                      | 결과                                                                                                                                                                     |
| ------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| 명목 주기 51.5초 + 회차별 sync | 12개 표시, phase 진입 1회. 다음 No Man's Land가 52.134초에 예측되고 실제 관측은 52.153초다. ID 대안·동시 파동을 처리하며 ambiguous·unresolved·violation·skipped는 없다.  |
| 다음 경계 관측 없이 전망       | 첫 회차 뒤 관측을 제거하고 종료를 180초로 연장한 합성 검사에서 시작 이벤트가 0 / 52.134 / 103.634 / 155.134초에 표시된다. 다음 시전을 기다려야만 반복하는 구조가 아니다. |
| 필수 관측 누락                 | 별도 순서 감사에서 본문 단계나 중간 경계를 제거하면 완주 회차로 인정하지 않는다. 실행기의 미래 표시 성공을 누락 없는 로그 검증으로 간주하지 않는다.                      |

이 검사는 실행 의미가 존재함을 확인한 기준 fixture다. 아래 자동 생성 검증과 구분한다.

## 자동 생성 첫 구현

유한 KDL을 압축하지 않고 본체 시도별 원본 파동에서 같은 opener와 말단 순환을 찾는다. 두 trace 이상에서 전체 본문과 다음 경계·후속 두 단계가 필요하다. 관측된 위치 교환은 연결 성분으로 블록화하며 고정 공통 신호와 중립 대안 표시를 분리한다. 특정 던전·능력 ID·명목 주기를 생산 코드에 넣지 않는다.

가상 helper가 `subType=Boss`로 표시된 보고서도 확인했다. 기존 입력 검사의 가상 actor 판정을 재사용해 본체 시도에서만 제외하고, helper 시전은 원본과 본문 대응에 보존했다. 원본 멤버·actor·instance·실제 시각·회차와 출력 노드 대응은 evidence에 남긴다.

| 코퍼스                              | 현재 정의 / loop | 판정                                                                 |
| ----------------------------------- | ---------------- | -------------------------------------------------------------------- |
| Alexandria 5개                      | 49 / 0           | 기존 유한 출력 유지, 반복 학습 미완료                                |
| Mistwake 8개                        | 87 / 0           | 기존 유한 출력 유지, 반복 학습 미완료                                |
| Meso Terminal 확장 15개             | 39 / 1           | Chirurgeon General 7위치 반복, 다른 보스는 유한 출력                 |
| Meso Terminal 보고서 보류 13개 학습 | 49 / 1           | `GrByCA3mZvDa2jLM`의 두 파일 전체를 학습에서 제외해도 같은 구조 생성 |

Meso 본문의 세 참조 일치 기록에서 원본 파동 대응은 각각 **4/8 → 8/8**이다. 두 Sterile Sphere ID의 원본을 보존한 채 위치 교환 블록을 `Pungent Aerosol / Sterile Sphere`로 표시한다. 선택이 확정됐다는 뜻이 아니며 각 위치에 가능한 표시를 공유한다. 출력 정의 수는 7개지만 여러 회차의 원본 발생을 모두 연결한다.

생성 회차는 참조의 No Man's Land보다 앞선 **Concentrated Dose**에서 시작하며 `start=60.0 every=48.5`다. 첫 진입 뒤와 다음 회차에서 일부 간격이 약 3초 달라진다. 같은 cyclic 구조를 다른 위치에서 시작한 데 따른 근사 시간이며, 참조의 51.5초를 생산 코드에 강제하지 않는다. 이후 No Man's Land 등 실제 관측으로 회차와 다음 회차 시계를 보정한다. 같은 ID를 쓰는 위치들의 window가 겹치면 해당 sync를 만들지 않는다.

| 생성 KDL의 별도 투영 재생     | 결과                                                                                                                                                                                                         |
| ----------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| 전체 15개 본체 범위           | 반복 본문의 원본 멤버 132개에 모두 표시 예측이 존재. 같은 신호의 가장 가까운 표시와 최대 차이 3,019ms. 5초 초과 0개, ambiguous·unresolved·violation·skipped 0개                                              |
| 보고서 전체 보류 후 15개 범위 | 같은 구조로 반복 생성. 전체 최대 차이 2,969ms, 예측 없음·5초 초과·진단 모두 0개                                                                                                                              |
| 그중 독립 보류 `/131`         | 원본 멤버 16개, 최대 차이 2,923ms                                                                                                                                                                            |
| 그중 독립 보류 `/446`         | 원본 멤버 7개, 최대 차이 337ms                                                                                                                                                                               |
| 기존 생성 회귀                | R9S 82개·R10S 155개·R11S 179개·Clyteum 3입력 87개 정의. 각각 기존 최신 기준 KDL과 바이트 단위 동일                                                                                                           |
| 합성 회귀                     | opener 없음·있음, dungeon·alliance에서 자동 loop 생성·원본 노드 대응·다음 관측 없는 미래 회차 표시. opener·본문·경계 누락, 불완전 순서 교환·같은 trace의 중복 지지를 거부. 시간 스케일 변형도 같은 구조 유지 |

위 시각 비교는 일대일 경로 정합성 검사가 아니다. 공통 구조를 먼저 확인한 뒤 각 원본 멤버에 같은 신호의 가장 가까운 표시가 있는지 측정하며 같은 파동의 복수 멤버는 한 표시를 공유할 수 있다. 5초는 검토 보고서의 진단 기준이고 생성기의 구조 승인 문턱이 아니다. 세 전체 본문의 ID·source·순서 감사와 함께 해석한다. 실패·완료 시도를 각각 나누어 재생했으므로 시도 간 전멸·재시작을 검증한 결과도 아니다.

## 같은 신호를 본문에서 재사용하는 반복

첫 구현의 ‘모든 위치의 신호 집합이 달라야 한다’는 제약을 제거했다. 회차 경계만 본문 안에서 고유해야 한다. 경계로 나눈 각 회차에서 같은 신호의 첫째·둘째 발생을 각각 첫째·둘째 위치로 대응한다. 개수와 다음 경계는 여전히 검사하므로, `A B C B D`를 `A B C D`로 줄이거나 B가 A를 대신하도록 허용하지 않는다. 고유 경계가 없는 반복은 아직 보류한다.

이 변경만으로 기존 Mistwake 8개 입력에서 Griffin의 21파동·31표시 정의 반복을 찾았다. 본체 이름을 쓰는 동시·보조 시전도 원본 멤버별로 보존한다. 회차 시작은 Golden Talons이며 명목 주기는 90.4초다. cactbot의 Fulgurous Fall 시작·88.9초와 기준점이 다르며, 실제 관측으로 회차를 보정한다.

일반화 검증을 위해 rankings page 1000, encounter 4549, difficulty 10에서 3개 보고서·4개 파일을 추가 수집했다. `out/mistwake-deep-plan.json`과 `.download.json`에 수집 근거가 있고 `logs/mistwake-longer`에 원본을 둔다. 전체 던전 길이는 819.810–937.246초, 이벤트는 합계 78,630개다. 이 네 파일을 기존 8개와 섞기 전에 기존 생성본의 보류 검증 대상으로 사용했다.

| 학습 → 보류        | 보류 본체 범위 / 원본 멤버 | 최대 예측 차이 | 예측 없음 / 5초 초과 / 진단 |
| ------------------ | -------------------------- | -------------- | --------------------------- |
| 기존 8개 → 새 4개  | 4 / 152                    | 924ms          | 모두 0                      |
| 새 4개 → 기존 8개  | 8 / 328                    | 994ms          | 모두 0                      |
| 기존 8개 자체 재생 | 8 / 328                    | 974ms          | 모두 0                      |

새 4개만으로도 같은 진행 구조와 주기 90.3초를 생성한다. 파일 단위가 아니라 서로 겹치지 않는 보고서 집합을 양방향으로 보류했다. 시각 평가는 위와 같은 최근접 동일 신호 비교이며, 일대일 경로 증명이나 ACT 검증은 아니다. opener·내부 단계·경계 누락, 개수 대체와 ID 충돌은 별도 합성 회귀로 검사한다.

| 생성 입력           | 전체 정의 / loop | 산출물                                                              |
| ------------------- | ---------------- | ------------------------------------------------------------------- |
| Mistwake 기존 8개   | 93 / 1           | `out/mistwake-context-loops.{kdl,evidence.json}`                    |
| Mistwake 추가 4개   | 99 / 1           | `out/mistwake-context-independent.{kdl,evidence.json}`              |
| Mistwake 전체 12개  | 95 / 1           | `out/mistwake-context-expanded.{kdl,evidence.json}`                 |
| Meso 기존 확장 15개 | 39 / 1           | `out/meso-context-loops.{kdl,evidence.json}`; 기존 KDL과 동일       |
| Alexandria 기존 5개 | 49 / 0           | `out/alexandria-context-loops.{kdl,evidence.json}`; 기존 KDL과 동일 |
| Clyteum 기존 3개    | 87 / 0           | `out/clyteum-context-loops.{kdl,evidence.json}`; 기존 KDL과 동일    |

**Griffin의 전체 cactbot 본문 일치 판정은 여전히 0회다.** Storm Surge 뒤 `Shock Storm`의 Electrifying Flight `B0F4`는 원본 cast에 존재하지만 본체와 이름이 다르고 FFLogs의 `enemyNPCs`에도 없어 현재 활동 후보·진행 그래프부터 빠져 있다. 생성기에서 본체 이름의 모든 후반 관측에 대응했다는 사실로 이 helper 누락을 숨기지 않는다. 참조 본문에 필요한 나머지 관측 대응과 helper 소속 연결이 후속 작업이다. 시도별 분리 재생은 처치·리셋·다음 보스까지 이어지는 lifecycle 검증도 아니다.

새 입력을 단독 생성할 때는 별도의 기존 ID 버그도 재현됐다. 구간별 참조 로그가 달라 같은 능력의 서로 다른 발생에 `-03`이 중복됐다. 원본 대응을 바꾸지 않고 충돌하는 이름에만 `after-a941-before-af19` / `after-a947` 같은 기준점 문맥을 붙였다. 합성 자료에서도 서로 다른 두 발생의 보존과 입력 순서에 무관한 KDL을 검사한다.

## 재현과 후속 구현

```sh
bun scripts/audit_dungeon_loops.ts \
  out/meso-terminal-long-loop-audit.evidence.json \
  /path/to/cactbot/ui/raidboss/data/07-dt/dungeon/meso-terminal.txt \
  /path/to/cactbot -o out/meso-terminal-long-reference-loops.json
cargo test --test dungeon_loop_reference
cargo test --test generate_loops
bun scripts/replay_dungeon_loops.ts \
  out/meso-generated-loop-final.evidence.json \
  out/meso-generated-loop-final.evidence.json \
  target/debug/btimeline out/meso-generated-loop-final-replay.json
bun scripts/replay_dungeon_loops.ts \
  out/meso-generated-loop-holdout-final.evidence.json \
  out/meso-generated-loop-final.evidence.json \
  target/debug/btimeline out/meso-generated-loop-holdout-final-replay.json
bun run --cwd scripts test
```

```sh
btimeline generate logs/loop-mistwake/*.json --mode dungeon -o out/mistwake-context-loops.kdl
btimeline generate logs/mistwake-longer/*.json --mode dungeon -o out/mistwake-context-independent.kdl
btimeline generate logs/loop-mistwake/*.json logs/mistwake-longer/*.json --mode dungeon -o out/mistwake-context-expanded.kdl
bun scripts/replay_dungeon_loops.ts out/mistwake-context-loops.evidence.json out/mistwake-context-expanded.evidence.json target/debug/btimeline out/mistwake-context-holdout-replay.json
bun scripts/replay_dungeon_loops.ts out/mistwake-context-independent.evidence.json out/mistwake-context-expanded.evidence.json target/debug/btimeline out/mistwake-reverse-holdout-replay.json
```

조사 당시 산출물은 `out/{alexandria,mistwake,meso-terminal,meso-terminal-long}-loop-audit.{kdl,evidence.json}`이다. 첫 구현은 `out/meso-generated-loop-final.{kdl,evidence.json}`, 보고서 보류 생성은 `out/meso-generated-loop-holdout-final.{kdl,evidence.json}`과 각 replay 보고서다. 원본은 CLI의 일반 `generate --mode dungeon` 입력이며 cactbot은 검증 oracle에만 사용한다. 전체 입력은 `logs/loop-meso-terminal/*.json`과 추가 파일 `logs/dungeon-loop-long/3Cy9dQN21DKYLABg_14.json`이며 보류 학습에서는 `GrByCA3mZvDa2jLM_*.json` 두 개를 제외한다.

| 후속 작업             | 제약                                                                                                                                                                                |
| --------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 반복 구조를 우선 학습 | 출력된 유한 이벤트를 사후 압축하면 이미 빠진 본문을 복원하지 못한다. 모든 원본 발생과 회차 대응에서 공통 본문을 찾아야 한다.                                                        |
| 진입·회차와 대안      | 초기 유한 opener와 뒤의 반복을 구분한다. 동일 ID의 내부 재사용·대안 ID·helper를 포함한 전체 진행 문맥으로 경계를 잡는다. 전역 DFG 순환이나 동일 ID 개수만으로 loop를 만들지 않는다. |
| 별도 시간 추정        | 51.529–52.153초의 차이만으로 같은 반복 구조를 버리지 않는다. 회차별 원점을 기준으로 주기·표시·보정 근거를 추정한다.                                                                 |
| 독립 검증             | 긴 보류 로그의 전체 회차와 다음 prefix, 살아 있는 범위의 시작·본문 누락, 짧은 처치, 보스 경계 reset을 구분한다.                                                                     |
| 종료와의 관계         | 무한 반복은 상위 구간이 끝날 때까지 유지한다. 보스 처치 후 예측 취소는 별도 종료 신호 문제이며, 해결책을 유한 관측 횟수로 고정해 우회하지 않는다.                                   |

전체 9개 보스에 대한 자동 loop 생성, 고유 경계가 없는 반복, 선택 이력에 의존하는 반복, 신호 탈출·보스 처치 종료 자동 생성, 실제 ACT 실행, 전체 실패 시도·재시작, StartsUsing·commented 단계와 모든 helper 대응은 아직 검증하지 않았다. 보스 처치 종료가 생성되지 않으므로 다음 phase 진입 전까지 불필요한 반복 표시가 이어질 수 있다.
