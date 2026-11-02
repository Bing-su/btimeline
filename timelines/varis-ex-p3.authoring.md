# Varis EX P3 작성 기록

2026-11-02. [timeline-author skill](../.agents/skills/timeline-author/SKILL.md)로 P3의 무작위 Gunshield 세 구간과 공통 후속 구간을 작성했다. P1·P2의 예고와 P4 이후 전체 진행은 범위 밖이다.

| 항목         | 내용                                                                                                                                                                                                                  |
| ------------ | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| KDL          | [varis-ex-p3.kdl](varis-ex-p3.kdl), 74개 이벤트·14개 phase                                                                                                                                                            |
| 후보 SHA-256 | `98aff0a0a422bd3b0950b3ebb2fe3518cc69e22e48c4c5d29367753476fc03f4`                                                                                                                                                    |
| 개발 입력    | [개발 계획](../tests/fixtures/authored/varis-p3-development-plan.json)의 기존 Varis long 14개. 모두 과거 조사·생성에 사용했으므로 개발 자료                                                                           |
| 대표·근거    | `Hpfdk4naLwANX62D/9`: P3 시작 `#9666`, E→L→R 선택 `#10177/#12256/#14148`, 공통 후속 `#16280`. 같은 보고서의 `/5`는 E→R→L을 지지                                                                                       |
| 참고 구현    | cactbot `e79ea7f16093be5b80cda0e2d5bc8eb899d6fb4b`의 [Varis EX](https://github.com/OverlayPlugin/cactbot/blob/e79ea7f16093be5b80cda0e2d5bc8eb899d6fb4b/ui/raidboss/data/05-shb/trial/varis-ex.txt)를 로컬 소스로 확인 |

## 해석과 실행

| 결정             | 이유·영향                                                                                                                                                                |
| ---------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| P3 진입          | Bladesblood의 Ability 4CCF를 start와 ready 진입에 사용. 앞선 P1의 같은 Gunshield 시전은 시작 신호로 사용하지 않음                                                        |
| 선택 관측        | Varis의 StartsUsing 4CD8/4CD9/4CD7이 L/R/E를 확정. 약 5초 뒤 Gunshield 해결과 이후 기믹을 예고                                                                           |
| 선택 이력        | 첫 선택 3개·두 번째 접두부 6개·마지막 블록 3개를 phase로 표현. 사용한 블록을 다시 고르지 않으며 같은 마지막 블록의 완료 상태만 공유                                      |
| 여섯 번째 순서   | 개발 자료의 LRE·LER·REL·ELR·ERL을 직접 관측. RLE는 세 블록을 한 번씩 사용한다는 전투 규칙에서 추론한 경로로 합성 검사하며 실제 관측과 구분                               |
| 시간과 창        | 블록 상대 시각은 개발 관측의 하위 중앙값을 0.1초로 반올림. 다음 선택 창은 L 53.0초·R 55.0초·E 58.7초 각각 ±15초. 단일 전역 평균 창으로 합치지 않음                       |
| 동기화           | L의 Magitek Burst, R의 Altius, E의 Magitek Shock를 대표 복구 신호로 사용. 원본 후속 패킷 모두를 sync로 만들지 않음                                                       |
| 누락·잘못된 선택 | 중간 선택이 빠졌거나 사용한 블록이 반복되면 알려진 경로로 대체하지 않음. 고유한 Ability 4CE0이 오면 현재 상태와 관계없이 joined로 복구                                   |
| 공통 후속        | Gunshield의 Magitek Spark / Magitek Torch는 두 시점의 대안 표시. 순서·자리·개별 대상은 예측하지 않으며 별도 선택이라고 확정하지 않음                                     |
| source 표기      | FFLogs의 `Varis yae Galvus`, `bladesblood`, `gunshield`를 참조의 `Varis Yae Galvus`, `Bladesblood`, `Gunshield`로 명시적 투영. ACT 이름 동등성을 실제로 검증한 것은 아님 |

표시는 대표 기믹 단위다. Alea Iacta Est의 연속 타격, 대상별 패킷과 동반 발동은 첫 대표 표시로 요약했다. 미래의 미선택 블록을 표시하지 않는다. 공통 후속 신호 이후의 두 대안 표시는 그 시점의 순서 미확정을 그대로 표현한다.

## 개발 검증

| 검사      | 결과                                                                                                                                  |
| --------- | ------------------------------------------------------------------------------------------------------------------------------------- |
| 입력      | 14개 중 적격 8개, P3 진입·공통 후속 근거가 없는 6개 제외                                                                              |
| 실제 순서 | 5종, 각 입력에서 ready→첫 선택→두 번째 이력→마지막 블록→joined 확인                                                                   |
| 예고 대상 | 152/152건, 최대 조회 오차 684ms                                                                                                       |
| Gunshield | 24건은 시전 시작으로 선택을 확인한 뒤 3초 전 조회. 실제 cast가 5초보다 약간 일찍 해결되므로 미선택 상태의 5초 전 조회를 강제하지 않음 |
| 후속 기믹 | 128건은 5초 전 조회. 이후 원본 관측을 포함한 기록을 주되 cutoff 이후는 실행에 사용하지 않음                                           |
| 합성      | 여섯 순서 × 선택 간격 80/100/120%, 선택 신호만으로 후속 예고·공통 합류. 중간 선택 누락·이미 사용한 선택·P1 신호의 P3 오진입도 검사    |

P3 시작과 공통 후속 4CE0이 관측되고 세 Gunshield가 각각 한 번씩 나타난 입력을 평가한다. 이미 합류 신호를 봤더라도 공통 후속 두 기술 전에 전투가 끝난 입력의 미관측 기술을 실패로 단정하지 않는다. FFLogs cast/begincast를 Ability/StartsUsing으로 투영한 검사이며 실제 ACT·UI·종료 신호는 미검증이다.

```sh
cargo build
target/debug/btimeline validate timelines/varis-ex-p3.kdl
bun scripts/verify_authored_varis_p3.ts target/debug/btimeline tests/fixtures/authored/varis-p3-development-plan.json out/varis-authoring/development-new.json
bun run --cwd scripts test
cargo test --test authored_varis_p3
```

출력은 새 경로만 허용한다. 입력 해시 변경·검사 도구 오류와 실제 예고·제어 실패는 평가 실패로 남기며 확인된 입력 결함만 제외한다. 사용한 보류 자료로 후보를 수정하면 해당 자료를 개발 자료로 전환한다. 후보의 독립 평가에는 내용 확인 전에 고정한 새 자료가 필요하다.
