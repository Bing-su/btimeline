# 참조 타임라인 없는 전투 작성

목표는 외부 시간표가 없어도 원본 로그에서 주요 진행을 해석하고 독립 자료로 예고를 확인하는 것이다. 절차와 실행 경계는 [에이전트 작성 합의](AGENT_TIMELINE_AUTHORING.md)를 따른다.

| 후보                       | 자료·현재 범위                                                                                                                                                      |
| -------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Clyteum                    | [작성 KDL](../timelines/clyteum.kdl)·[해석 기록](../timelines/clyteum.authoring.md). 원본에서 세 보스의 도입·ID 대안·반복을 작성                                    |
| 산도리아: 두 번째 반향세계 | Game zone 1304. [수집 계획](../tests/fixtures/authored/sandoria-download-plan.json)에 학습 후보 4개 전투의 메타데이터를 확보. 이벤트 해석·KDL 작성은 아직 하지 않음 |

## 산도리아 입력 후보

| encounter | 보스                                  | report/fight          |
| --------- | ------------------------------------- | --------------------- |
| 2067      | Faithbound Kirin                      | `PAq8bNtxvXWG16QZ/68` |
| 2068      | Omega, the One and Ultima, the Feared | `PAq8bNtxvXWG16QZ/70` |
| 2069      | Kam'lanaut                            | `PAq8bNtxvXWG16QZ/72` |
| 2070      | Eald'narche                           | `PAq8bNtxvXWG16QZ/73` |

후보 네 전투는 같은 보고서이므로 독립 표본 네 개로 세지 않는다. 수집 후 다중 보스 동시 진행·반복·phase 전환을 먼저 확인하고 작성 범위를 고정한다. 별도 보고서를 보류 평가용으로 확보한다.

```sh
btimeline fflogs-download tests/fixtures/authored/sandoria-download-plan.json -o logs/sandoria-authoring-training
```

원본의 모든 활동을 표시로 만들지 않는다. 이름·시전자·반복 문맥으로 대표 기믹과 동반 발동을 구분하고, 위치·선택 관계를 확정할 수 없으면 그 해석은 보류한다. 시간표가 없는 것을 추정의 확실성으로 대체하지 않는다.
