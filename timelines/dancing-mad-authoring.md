# Dancing Mad (Ultimate) 작성 기록

[실행 KDL](dancing-mad.kdl)은 FFLogs encounter 1085, difficulty 100의 Kefka 도입부터 최종 Forsaken Null 광폭화까지 다룬다. 기존 개발 코퍼스 `logs/dancingmad/`의 56개 pull을 사용했다. 모두 과거 조사에 쓰인 자료로, 이번 결과에서 독립 보류로 세지 않는다.

기믹명·상대 일정의 초안은 cactbot의 [Dancing Mad 원본](https://github.com/OverlayPlugin/cactbot/blob/e79ea7f16093be5b80cda0e2d5bc8eb899d6fb4b/ui/raidboss/data/07-dt/ultimate/dancing_mad.txt)을 참고했다. 이를 phase 상대 시간의 KDL로 재작성하고, FFLogs의 시전 관측과 다음 phase까지의 진행을 대조했다. cactbot 부분의 저작권은 [cactbot 기여자](dancing-mad-apache-license.txt)에게 있으며 Apache-2.0 조건을 따른다.

## 작성 범위

| 구간             | 진입 신호                | 주요 일정                                                                                                              |
| ---------------- | ------------------------ | ---------------------------------------------------------------------------------------------------------------------- |
| P1 Kefka         | `C403` StartsUsing       | Revolting Ruin, Graven Image, Mystery Magic, Hyperdrive, Tele-trouncing과 Light of Judgment                            |
| P2 God Kefka     | P1 뒤 `C24C` StartsUsing | Ultimate Embrace, Forsaken의 네 세트, Trine과 Wings of Destruction                                                     |
| P3 Chaos·Exdeath | P2 뒤 `C3F7` StartsUsing | Aero III Assault 전환, 원소 기믹, Ultima Blaster, Earthquake·Nothingness, 후반 Blizzard·탑과 Big Bang                  |
| P4 Grand Cross   | P3 뒤 `C2DC` StartsUsing | Kefka Says, Grand Cross 세 번, Flood of Naught, Death Surge, Mana Charge/Release                                       |
| P5 Final Kefka   | P4의 후반 `C3FD` Ability | Ultima Upsurge, Ultima Repeater, Fell Forces, Flood, Celestriad, Stray Apocalypse·Entropy, 네 Forsaken과 Forsaken Null |

`BABB`와 `BAE1`은 해당 phase의 광폭화 일정으로 전환한다. P3의 두 보스가 광폭화를 시작하면 Chaos와 Exdeath의 sequence를 각각 예약한다. `C61E` 또는 `C61F`로 확인하는 단독 광폭화는 생존자별 phase에서 실행한다. 생존자가 발동 전에 처치되면 `WasDefeated`로 예약을 닫고, 이후 `C2DC`에서 P4를 다시 연다. P5에서 Kefka 처치나 전투 종료를 받으면 남은 예고를 닫는다.

Spelldriver·Spellscatter·Spellwave, 원소 순서, 일부 디버프 효과는 공통 예고나 대안 표기로 표현한다. 위치·대상·실제 분기 선택을 추정하지 않는다. 일부 표시는 참조 일정의 타깃 가능 여부와 디버프 예고이므로 개별 FFLogs 시전에 대응하지 않는다. 원본 시전을 모두 별개 표시로 만드는 대신 같은 웨이브의 중복 패킷과 동시 효과를 구분한다.

## 원본 근거와 재생 경계

- 정상 진행 예시 `BzRGwA17mvnqF2K9/18`에서 #701 `C403`, #11625 `C24C`, #21024 `C3F7`, #39500 `C2DC`, #46471 `C3FD`가 각 구간을 연다. P5의 #59918은 `BB3A` 시작이다.
- `8CXqhmj4wnbYTyHr/2`의 #18311 `BAE1` 뒤 #18898 `C403`은 병합된 재시도다. `VBPGrcqXAgvt24KQ/3`의 #16872는 앞선 P2 뒤에 다시 관측된 P1 시전이다.
- `CNfj8F9kGQdBaPgR/91`은 P1 도중 파티 사망과 구역 경계 뒤 #8841에서 새 P1 시전이 기록됐다. 원본 시전만 투영한 재생에는 초기 P1을 닫을 ACT 신호가 없다. 별도 재생에서 관측된 FFLogs 구역 경계에 `ChangeZone`을 합성해 재시작을 검사했다.
- 단독 Exdeath 광폭화는 개발 입력 21개에서 발동 전에 HP=0이 관측됐다. `WasDefeated`를 그 위치에 합성하는 취소 검사를 별도로 수행했다. FFLogs HP=0이나 zonechange를 외부 실행 조건으로 KDL에 넣지 않았다.

## 개발 검증

KDL SHA-256은 `82dbbc7592a81e05db5fbb3f0279400e2edb515aaae0a1d05bb8ba067c1ebd6d`다. 구조 검사에서 이벤트 정의 348개를 확인했다.

| 검사                           | 결과                                                                                                                                                                                                                                                    |
| ------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 시전만 투영한 56개 pull        | 클리어 16개 모두 P5에 진입했다. 전체 P5 진입은 25개였으며 모호성·미해결·위반 진단은 0건이었다.                                                                                                                                                          |
| 개발 표본 8개의 주요 효과 조회 | P1 44건, P2 56건, P3 48건, P4 42건, P5 23건, 단독 Chaos 광폭화 1건이 대응했다. 총 214건이 1.5초 허용 오차 안에 들었다. 최대 오차는 1,048ms였다. 효과 3초 전, Aero III Assault는 2초 전에 조회했다. 한 표본에는 위에서 설명한 합성 구역 이탈이 포함된다. |
| HP=0 위치의 합성 처치          | P3 단독 광폭화 21건과 P5 종료 13건에서 예약을 취소했고, 다음 phase가 관측된 경우 진입을 유지했다.                                                                                                                                                       |
| 별도 합성 제어                 | 초반 광폭화 후 재시작, P2 병합 재시도, P3 양쪽 및 각 단독 광폭화, 단독 처치 후 P4, P4 광폭화, P5 처치 및 광폭화의 9개 사례를 통과했다.                                                                                                                  |

`out/dancing-mad/`에 입력 검사, 원본 파일 해시와 phase 진입 인덱스, 정규화 기록, 예고 표본, 합성 처치 결과를 저장했다. 위 예고 수는 선택한 주요 효과에 한정한다. 미관측 분기, 개별 대상이나 위치, 새 보류 로그, 실제 ACT 신호 및 외부 실행기 동작은 확인하지 않았다.
