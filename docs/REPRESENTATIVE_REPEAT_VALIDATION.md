# 대표 그래프 말단 반복 검증

2026-11-02. 말단 반복의 첫 그래프 통합을 검사했다. 합성 자료의 KDL 단독 예고와 실제 자료의 생성 회귀를 구분한다. ACT·외부 실행기·게임 실행은 미검증이다.

## 합성 자료

`cargo test --test generate_loops`의 다음 사례는 생성기에 직접 FFLogs fixture를 넣고 검사한다. 입력은 테스트가 결정적으로 생성한다. 별도 검증 입력을 학습에 섞지 않으며 자동 보고서 보류 검증으로 취급하지 않는다.

| 사례                | 확인 결과                                                                                                                       |
| ------------------- | ------------------------------------------------------------------------------------------------------------------------------- |
| 긴 전멸 두 개       | 반복 파동 7개/11개와 내부 순서 변형을 한 `repeat`로 연결. raid·dungeon·alliance 지원                                            |
| opener 없는 보스    | 첫 반복 경계를 phase 진입으로 사용. 유한 경계는 loop로 대체                                                                     |
| 원본 보존           | 유한 경로와 `observed_waves` 참조의 합집합이 모든 실행 대상 발생을 보존. 마지막 부분 회차에 가짜 참조 없음                      |
| 입력 순서           | 역순 입력의 KDL과 바이트 단위 동일                                                                                              |
| 미관측 다음 회차    | evidence를 삭제하고 시작 관측만 공급. 학습 전투의 종료 이후인 20초 뒤에도 이벤트를 예고하며 ambiguous·unresolved·violation 없음 |
| 선택 후 반복 합류   | B→C / C→B를 선택한 뒤 공통 말단 반복으로 진행. raid·dungeon에서 선택 신호만으로 후속 일정과 미래 회차를 예고                    |
| 반복 시간 추정 실패 | 구조상 반복이어도 추정된 주기보다 본문이 길면 실패. KDL과 evidence 모두 저장하지 않음                                           |

## 실제 코퍼스 생성 회귀

모든 입력은 기존 학습용 파일이다. 이번에는 별도 holdout·원본 투영 재생·cactbot 비교를 수행하지 않았다. 두 묶음 모두 `generation_method=common-backbone-v1`, 생성 loop와 대표 `repeat`는 0개였다. 따라서 아래 성공은 새 반복 경로의 실제 자료 검증을 뜻하지 않는다.

| 묶음    | 명령                                                                                                                            | 결과 KDL SHA-256                                                                |
| ------- | ------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------- |
| R11S    | `btimeline generate logs/r11s/{4kt1bfqJn7QPCj29_2,c36H8h2aqXwkgtjR_3,fDhKygMQ3VCvZat8_2}.json --mode raid -o r11s.kdl`          | `2a11957af456818be2d7c4c44328629fbb1fbd9a01904681d13db5c4d2bf13cc` (187개 정의) |
| Clyteum | `btimeline generate logs/clyteum/{ZpHDQcCnfgBtaTPL_1,TwQYZydBhgLfn3bC_1,tpmrkL76NhXHMbZv_7}.json --mode dungeon -o clyteum.kdl` | `0280aa0adbeba0eea01cfbe6d069739b6bc294a718ba98549bd801983399ccb0` (65개 정의)  |

명령의 `{…}`는 zsh/bash의 brace expansion이다. 생성 시 출력 경로는 비어 있어야 한다.

| 입력 파일                              | SHA-256                                                            |
| -------------------------------------- | ------------------------------------------------------------------ |
| `logs/r11s/4kt1bfqJn7QPCj29_2.json`    | `475d3009e53ff78bfc2cdf65021cbd7aeca45800110a4cd3116a3bb322e39bd4` |
| `logs/r11s/c36H8h2aqXwkgtjR_3.json`    | `d3f1f8a4d61c9486a0d98c31fd9e52ce30bd24f460cea3a20f90433fa8ebef61` |
| `logs/r11s/fDhKygMQ3VCvZat8_2.json`    | `d43354d67eb0c186c64d24889321c290ceb7d7b3e5fa01d6038a6d2e76ed66c3` |
| `logs/clyteum/ZpHDQcCnfgBtaTPL_1.json` | `785cf97ddbccc0c870c98485d1ef907d2780a58279cf962472b63f38efadd5fb` |
| `logs/clyteum/TwQYZydBhgLfn3bC_1.json` | `10f2d5bf0f58718d7874685ee391501e87429a2287d724c68fc4fadb5903744b` |
| `logs/clyteum/tpmrkL76NhXHMbZv_7.json` | `b36a637edccc1bb2659b43917cbfc1a852a266f4d847b8e03149ed4304c2b2df` |
