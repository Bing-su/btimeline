# 타임라인 v2 계약

KDL 2 문서의 루트는 `timeline 2 "이름"`이다. `timeline 1`, 작성 구문 `sequence`, `branch at`, 직접 실행 본문을 가진 phase는 지원하지 않는다. `mine`의 `process 1`과 정규화 recording의 `version: 1`은 별도 프로토콜이므로 유지한다.

## 구조와 시계

| 노드                            | 위치와 의미                                                                                    |
| ------------------------------- | ---------------------------------------------------------------------------------------------- |
| phase                           | root. 하나 이상의 step을 묶고 자체 시계는 없음                                                 |
| step                            | phase. enter/after로 진입하는 배타적 진행 위치. 이전 step과 하위 작업 취소                     |
| event, sync                     | root 및 일정 본문. root는 fight 시계                                                           |
| section ID at=T                 | step/reaction/option의 고정 offset 그룹. namespace나 실행 instance 없음                        |
| reaction                        | step. on 관측에서 독립 시계 시작. 같은 reaction 재관측은 미실행 일정을 교체. event/sync만 소유 |
| choice ID at=T                  | step당 최대 하나. option 관측이 부모 step을 닫고 독립 시계를 시작                              |
| branch ID open=T close=T        | step/loop. 부모 시계와 진행을 유지하는 선택. 최소 두 option과 선택적 merge                     |
| loop ID start=T every=T count=N | step 또는 일반 branch option. 고정 간격 반복과 선택적 양의 정수 count. 중첩 금지               |

phase의 직접 enter/event, 중첩 phase/step/choice, reaction 내부 제어는 금지한다. section은 부모의 허용 본문을 넓히지 않는다. 동시 loop 본문은 step당 하나다. 반복 이후 복구 일정은 별도 step에 쓴다. on-exit은 지원하지 않는다.

```kdl
timeline 2 "Example" {
    start Pull
    wipe Wipe
    sync 2 Anchor before=1 after=1
    phase fight {
        step opening {
            enter Pull
            choice dodge at=5 {
                shared { event common 3 name="Common"; }

                option left { when Left; event hit 4 name="Left"; }
                option right { when Right; event hit 4 name="Right"; }
                join fight/end after=6 recover-nominal=#false { when Join; }
            }
        }
        step end { event hit 1 name="End"; }
    }
}
```

## 참조와 저작 정보

local ID에는 `/`를 금지한다. 예약 segment shared/reaction/loop는 생성 경로와 충돌하는 위치에서 금지한다. section 이름은 identity에 참여하지 않으며 반복해도 된다.

| 정의                     | 정규 참조                                           |
| ------------------------ | --------------------------------------------------- |
| root event               | event                                               |
| step event               | phase/step/event                                    |
| choice option event      | phase/step/choice/option/event                      |
| shared event             | phase/step/choice/shared/event                      |
| reaction event           | phase/step/reaction/id/event                        |
| loop event               | owner/loop/id/event                                 |
| 일반 branch option/merge | owner/branch/option/event, owner/branch/merge/event |

공통 shared는 명목/선택 경로에서 같은 정의 ID다. 실행 발생은 run, phase_instance, iteration, sequence_instance와 정의 ID로 구별한다. Rust 출력의 phase는 실행 step/choice option을, sequence는 reaction을 뜻한다. 옵션 적용 소유권과 원본 정의 위치를 함께 제공한다.

컴파일은 위치를 가진 원본 AST를 한 번 읽고 정규화·검증한다. 실행 중간 형식화/재파싱은 하지 않는다. catalog에는 원문 UTF-16 source_span(file/start/end)과 declaration_order가 있다. 정렬용 실행 확장 순서와 원문 위치는 별개다. expand는 v2를 유지하는 별도 출력이며 이벤트 안 implicit sync를 명시하고 section offset을 전개한다.

## 시간과 제어

저작 시간은 유한한 음이 아닌 0.1초 단위, 관측은 원본 밀리초와 순서를 보존한다. 창은 시작 포함/끝 제외다. sync/choice 기본 창은 ±2.5초이며 명시할 때 before/after를 함께 쓴다. event의 `sync Ability ...`는 event 시각을 상속하고 `sync 2 Ability ...`는 명시 시각이다. 독립 sync는 항상 시각이 필요하다. section 이동은 event/sync를 함께 이동하며 ID는 유지한다.

필드 literal은 전체 일치, 한 field의 대안은 OR, 여러 field는 AND다. regex는 전체 필드를 매치하며 ignore-case를 지원한다. 예약 속성은 field 자식으로 매치한다. 알 수 없는 속성/노드/참조, 중복, 잘못된 시간·타입·중첩은 거부한다. regex 교집합 및 외부 입력 가용성은 파싱 성공만으로 증명되지 않는다.

start가 fight를 열어야 enter가 활성화된다. enter의 from은 정규 step/choice option 참조이고, 한 from 자식에 비어 있지 않은 서로 다른 참조를 나열할 수 있다. from-phase는 그룹의 모든 진행 위치를 명시적으로 허용하며 from과 배타적이다. choice 자체 ID는 진행 위치가 아니다. enter at/window는 source clock을 검사한다. resume-at은 활성 step이 없을 때만 복구하고 과거 작업을 건너뛴다. 같은 step의 반복 enter는 재시작하지 않는다.

after는 양의 시간과 단일 source를 요구하며 source당 timed successor는 하나다. 관측은 같은 시각의 예정 작업보다 우선하고 제어에 소비한 관측은 event sync로 재사용하지 않는다. sparse tick도 실제 논리 deadline에 전환한다. timed entry의 일치하는 늦은 enter는 한 번만 새 원점을 보정한다.

choice option은 관측 시각이 0초다. 미선택이면 shared 명목 일정과 실제 option의 조건부 대안을 보여주며 option을 추측하지 않는다. `preview` 선언은 거부한다. shared sync는 selected 경로에서만 활성화한다. 미선택 복구용 sync는 부모 step에 choice 시작 시각을 더한 절대 offset으로 선언한다. shared event 및 외부 실행기의 선행 cue는 선택 창 종료 이후여야 한다.

join은 명목 choice at+after 또는 선택 option 원점+after에 timed 진입한다. when은 기본 selected 경로만 허용한다. recover-nominal=#true일 때만 명목 관측 복구를 허용한다. target은 실제 step이며 when은 paired window 외에 from/at/resume-at을 바꿀 수 없다.

일반 branch의 requires는 최신 선택 이력을 AND로 검사한다. 미관측을 추측하지 않고, 자기/누락/순환/상충 참조는 거부한다. fallback은 선택 이후 듣지 않는다. option resume-at은 미선택에서만 적용하며 해당 option은 loop를 소유할 수 없다. merge는 부모 창에서 공통 관측으로 미선택 상태도 합류할 수 있으며 이전 option 작업을 취소한다.

loop의 직접 event/sync는 every 이전, branch close는 every 이하이고 늦은 선택·합류도 회차를 넘으면 안 된다. loop merge는 명시 유한 창을 요구한다. until은 at이 필요하며 after 생략 시 부모 종료까지 감시한다. 본문 시작 전에도 탈출할 수 있다. 반복 sync는 현재·후속 회차 원점만 보정하고 부모/exit 창은 유지한다.

동일 시계 sync는 절대 창 끝, 원본 선언 순서, 안정적인 확장 순서를 따른다. 서로 다른 시계의 sync 충돌은 그 보정만 생략한다. 전진 보정은 지나친 미실행 작업을 건너뛰고 정확한 도착 시각은 실행한다. 후진 보정은 이미 처리한 발생/회차/선택을 재실행하지 않는다.

## 중단과 투영

lifecycle 우선순위는 zone-exit, wipe, finish, boss-complete, start다. boss-complete는 step만 닫고 fight/history를 유지한다. wipe/finish/zone exit는 모든 실행을 닫는다. 제어 모호함 또는 동시 loop 위반은 timeline을 suspended로 만들고 해당 시각 이후 표시를 취소한다. wipe/reset 이후 새 start로 재개한다. sync 충돌은 중단하지 않으며 미선택 close는 unresolved다. 원본 입력 손상/순서 역행은 강한 오류다.

forecast는 cutoff까지 실제 관측을 재생한 뒤 읽기 전용 투영을 수행한다. 미래 실행, scope 생성, cue 소비, clock 변경은 하지 않는다. 현재 일정, 여러 반복 회차, timed successor를 탐색하며 미래 관측 선택/merge/exit는 추측하지 않는다. 예정 successor 계산을 실행기와 공유한다.

컴파일 결과는 choice ID, 부모 step, 명목 시각, option ID 관계를 보존한다. forecast의 `events`는 현재 실행 경로이고, `alternatives`는 별도의 조건부 일정이다. 각 대안은 `choice_id`, `option_id`, `at_ms`, `state`(waiting/open/unresolved), `events`, `omitted`를 갖는다. 대안 이벤트의 `route`는 unresolved, `origin_basis`는 scheduled다. shared와 공통 합류 일정은 events에 한 번만 표시한다. 선택되면 해당 choice 대안은 사라지고 관측 원점을 사용한다. 선택 창을 놓쳐도 재개방하지 않는다.

`--max-rows`는 events와 전체 대안 이벤트에 각각 적용한다. 대안은 각 option에 한 행씩 순환 배분하며 잘린 행 수를 option별 omitted로 알린다. 탐색 예산 소진은 전체 truncated/reason으로 알린다. evaluator는 alternatives를 채점하지 않는다. 모든 후보를 맞은 예측으로 간주하면 평가가 무의미해진다.

horizon-ms는 음이 아닌 유한 정수 시간이고 max-rows는 음이 아닌 정수다. 0은 빈 미래 목록이다. 기본 max-rows는 10000, 계산 budget도 10000이다. 정렬 후 row 제한을 적용하고 truncated와 truncation_reason을 반환한다. 평가기는 잘린 결과를 완전한 예고로 채점하지 않는다. projection_diagnostics는 비어 있으며 가상의 선택 창 닫힘을 실행하지 않는다.

events에는 origin_basis(observed/scheduled), route(common/selected/unresolved)를 넣는다. observed는 원점의 근거이며 미래 공격 자체가 관측됐다는 뜻이 아니다. 관측되지 않은 대안을 countdown으로 확정하지 않는다. Rust forecast는 `(at_ms, until_ms]`의 미래 visible 행을, Rangbot 표시기는 지속 중인 행과 상대 대안을 추가로 표시한다.

## 이관과 검증

[ID 변환표](timeline-v2-id-map.json)는 기존 정의와 정규 참조의 대응을 보존한다. 경로는 저장소 이름을 기준으로 하므로 다른 체크아웃에서도 사용할 수 있다.

평가 profile의 phase/events/소유 참조는 정규 ID로 이관한다. 원본 recording과 FFLogs는 변경하지 않는다. 과거 validation/holdout 보고서와 동결 manifest는 당시 해시의 역사적 증거다. 새 KDL에 해시만 덮어씌워 재사용하지 말고 새 manifest/출력으로 다시 평가한다. projection envelope는 원본 KDL SHA-256이 일치해야 한다.

생성기는 v2 step을 phase로 묶어 출력하고 evidence의 event/phase/branch/loop 참조를 같은 소유 경로로 변환한다. 파싱·합성 재생·생성 검사는 실제 ACT 입력과 게임 기믹 정확도 검증을 대체하지 않는다.
