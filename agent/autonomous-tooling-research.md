# 자율 타임라인 개선에 필요한 도구 조사

조사일: 2026-10-15 (Asia/Seoul). 범위는 준비 상태와 필요한 도구 조사다. FFLogs 인증 요청·로그 수집·설치·생성 알고리즘 변경은 수행하지 않았다.

## 판단

새 MCP나 대형 분석 프레임워크를 추가할 근거는 없다. 기존 Rust FFLogs 클라이언트, 로컬 cactbot 소스, Nushell/Node 분석 스크립트를 연결하는 작은 실행 도구가 우선이다. 필요한 것은 **표본 발견과 수집 예산**, **cactbot 의미 비교**, **독립 검증 세트와 통합 판정**이다. 이는 아래 확인된 API·소스 동작을 바탕으로 한 구현 권고이며, 아직 구현된 기능이라는 뜻은 아니다.

## 공식 자료에서 확인한 사항

| 항목 | 확인 결과 | 작업에 주는 의미 | 출처 |
| --- | --- | --- | --- |
| 공개 로그 인증 | OAuth client credentials로 `/api/v2/client`에 접근한다. client ID/secret으로 토큰을 받는다. | 웹 브라우저 로그인 자동화 대신 기존 API 클라이언트를 사용한다. | [FFLogs API 인증 문서](https://www.fflogs.com/api/docs) |
| 비공개 로그 인증 | `/api/v2/user`는 사용자 동의를 받는 authorization code 흐름을 사용한다. | 공개 자료로 시작하는 데 비공개 OAuth 구현은 필요하지 않다. | [FFLogs API 인증 문서](https://www.fflogs.com/api/docs) |
| 보고서 탐색 | `reportData.reports`는 guild/user/date/zone 필터와 page/limit를 제공한다. 페이지 크기는 최대 100이다. 문서는 특정 guild/tag/user의 보고서 집합으로 설명한다. | 전 세계 모든 로그를 검색하는 기능으로 가정하지 말고, 지정 보고서·길드·작성자와 랭킹을 표본 발견 경로로 사용한다. | [ReportData 스키마](https://www.fflogs.com/v2-api-docs/ff/reportdata.doc.html) |
| 전투별 랭킹 | `Encounter.fightRankings`는 page, partition, difficulty, region, metric 등의 인자를 받는다. FF의 bracket은 패치다. 반환형은 JSON이다. | 패치·지역·클리어 시간에 분산된 후보를 찾는 수단이다. 실제 응답의 보고서/전투 식별자와 종료 조건은 첫 소량 조회에서 검증해야 한다. | [Encounter 스키마](https://www.fflogs.com/v2-api-docs/ff/encounter.doc.html) |
| 이벤트 조회 | `Report.events`는 전투·시간·이벤트 종류 필터를 받으며 `ReportEventPaginator`를 반환한다. 요청 limit는 100–10000이다. | 한 페이지로 수집 완료를 판단할 수 없다. 기존 클라이언트의 페이지 순회를 재사용한다. | [Report 스키마](https://www.fflogs.com/v2-api-docs/ff/report.doc.html) |
| API 사용량 | FFLogs의 `RateLimitData`에는 `limitPerHour`, `pointsSpentThisHour`, `pointsResetIn`이 있다. | 고정 요청 횟수 대신 실제 사용량을 관측하여 수집 중단·재개를 결정한다. | [FFLogs RateLimitData 스키마](https://cn.fflogs.com/v2-api-docs/ff/ratelimitdata.doc.html) |
| 오래된 자료 | archived report의 events/tables/graphs는 archive 접근 구독이 없으면 접근할 수 없다고 문서에 명시되어 있다. | 새 구독을 전제하지 않고 접근 가능한 보고서부터 선택한다. | [Report 스키마](https://www.fflogs.com/v2-api-docs/ff/report.doc.html) |
| FFLogs의 정보 한계 | cactbot 가이드는 FFLogs에 RP 문구·지역 봉인/해제 문구·새 combatant 등 일부 로그 줄과 능력이 없다고 설명한다. | 이런 동기화는 raw ACT 로그를 별도 확보하기 전까지 미검증으로 남겨야 한다. | [cactbot Timeline Guide](https://github.com/OverlayPlugin/cactbot/blob/main/docs/TimelineGuide.md#run-the-fight-a-few-times) |
| cactbot 자체 검사 | 공식 가이드는 `npm run test`와 `util/logtools/test_timeline.ts`를 안내한다. 후자는 실제 `Timeline` 컨트롤러를 구동한다. | 자체 Rust replay 외에 실제 소비자에서 검증할 수 있다. | [가이드](https://github.com/OverlayPlugin/cactbot/blob/main/docs/TimelineGuide.md#using-test_timelinets), [검사기 소스](https://github.com/OverlayPlugin/cactbot/blob/e79ea7f16093be5b80cda0e2d5bc8eb899d6fb4b/util/logtools/test_timeline.ts) |

FFLogs 문서의 직접 열기는 여러 URL에서 403을 반환했다. 위 FFLogs 항목은 웹 검색이 반환한 **공식 도메인 문서 본문**으로 확인했으며, 인증된 API 호출로 재확인하지 않았다. 특히 실제 계정의 할당량, 포인트별 비용, 유료 구독 가격, private/archive 접근 가능 여부는 확인하지 않았다. 시간당 제한이나 구독 가격을 임의의 숫자로 고정하면 안 된다. `ReportEventPaginator`의 최신 스키마 페이지도 직접 확인하지 못했다.

## 로컬 cactbot 소스로 확인한 재사용 도구

검사 위치: `/home/ks2515/workspace/cactbot`, HEAD `e79ea7f16093be5b80cda0e2d5bc8eb899d6fb4b`. origin은 사용자 fork, upstream은 `OverlayPlugin/cactbot`이다. 아래는 이 SHA의 소스 확인 결과다.

| 기존 도구 | 역할 | 적용 시 주의점 | 고정 소스 |
| --- | --- | --- | --- |
| `ui/raidboss/timeline_parser.ts` | 실제 타임라인 문법·동기화 파싱 | 문자열 diff보다 의미 비교의 출발점으로 적합 | [parser](https://github.com/OverlayPlugin/cactbot/blob/e79ea7f16093be5b80cda0e2d5bc8eb899d6fb4b/ui/raidboss/timeline_parser.ts) |
| `ui/raidboss/timeline.ts` | sync window·jump·타이머·미래 이벤트 표시 | 파싱 성공만으로 재생·예고 품질을 판정할 수 없음 | [controller](https://github.com/OverlayPlugin/cactbot/blob/e79ea7f16093be5b80cda0e2d5bc8eb899d6fb4b/ui/raidboss/timeline.ts) |
| `test/helper/test_timeline.ts` 및 `test/test_data_files.ts` | 문법·스타일·trigger 파일의 timeline 연결 검사, 파일 선택 실행 | 생성 `.txt`를 임의 경로에 두는 것만으로 공식 전체 검사를 바로 적용할 수 있다고 가정하지 않는다. trigger 경로/파일 연결 규약이 있음 | [timeline 검사](https://github.com/OverlayPlugin/cactbot/blob/e79ea7f16093be5b80cda0e2d5bc8eb899d6fb4b/test/helper/test_timeline.ts), [실행기](https://github.com/OverlayPlugin/cactbot/blob/e79ea7f16093be5b80cda0e2d5bc8eb899d6fb4b/test/test_data_files.ts) |
| `util/logtools/test_timeline.ts` | ACT 로그 또는 FFLogs 기반 실제 컨트롤러 검사 | timeline 이름으로 cactbot 내부 trigger/`.txt`를 탐색함. drift/missed 결과가 콘솔 중심이라 자동 판정용 구조화 결과는 별도 확인 필요 | [소스](https://github.com/OverlayPlugin/cactbot/blob/e79ea7f16093be5b80cda0e2d5bc8eb899d6fb4b/util/logtools/test_timeline.ts) |
| `util/logtools/fflogs.ts` | cactbot 자체 FFLogs 수집 | 현재 소스는 v1 API key 방식이다. btimeline의 v2 OAuth 자격증명을 그대로 넣는 대체 수집기로 취급하면 안 됨 | [소스](https://github.com/OverlayPlugin/cactbot/blob/e79ea7f16093be5b80cda0e2d5bc8eb899d6fb4b/util/logtools/fflogs.ts) |
| raidboss emulator | 실제 화면·타임라인 확인 | headless 수치 평가가 부족한 경우 브라우저 QA에 사용. raw ACT 입력이 있어야 검증 범위가 넓어짐 | [공식 README](https://github.com/OverlayPlugin/cactbot#raidboss-emulator) |
| Node/npm | 위 TypeScript 도구 실행 | 이 checkout은 Node `>=20.10.0`을 명시함 | [package.json](https://github.com/OverlayPlugin/cactbot/blob/e79ea7f16093be5b80cda0e2d5bc8eb899d6fb4b/package.json) |

## 추가 준비 권고 — 새 외부 제품보다 기존 코드 보완

| 우선순위 | 필요한 작은 도구/준비 | 완료 조건 예시 |
| --- | --- | --- |
| 1 | FFLogs 자격증명 실행 환경 설정 | 비밀 값을 출력하지 않고 토큰 발급과 소량 조회 성공을 확인. 현재 조사에서는 인증 요청하지 않음 |
| 1 | 보고서 발견 + 사용량/예산 + 재개 가능한 수집 드라이버 | API 포인트·보고서 수·디스크 상한에 따라 중단하고, 완료한 전투를 재다운로드하지 않으며 실패 목록을 보존 |
| 1 | 고정 평가 manifest | 입력 파일 hash·report/fight ID·패치·지역·kill/wipe·cactbot SHA·설정을 기록. 같은 report의 전투가 생성/평가 양쪽에 섞이지 않도록 분리 |
| 1 | 공식 parser 기반 참조 비교 | 로컬 encounter와 cactbot zone/파일 매핑을 명시. 이름 번역·ID 배열·burst duration·숨긴 sync·branch/jump를 정규화하여 차이를 분류 |
| 1 | 단일 평가 실행기 | 기존 raw coverage·Rust replay·실제 cactbot consumer·몇 초 전 표시되는지의 예고 지표·독립 report holdout을 함께 실행. 실패가 전체 성공으로 표시되지 않음 |
| 2 | 평가 범위 확장 | 기존 evaluator의 alliance 및 checkpoint 결합 전투 범위를 확인하고 해당 입력을 독립 검사. 레이드/던전 단일 encounter 통과만으로 전체 품질을 주장하지 않음 |
| 2 | 추가 표본 | 오래 끄는 전투, enrage, HP push, 분기, 체크포인트 직접 진입, wipe, 다른 보고서·작성자·지역을 보충. 랭킹 상위의 빠른 kill만 수집하지 않음 |
| 3 | raw ACT 로그 + 필요시 Playwright | FFLogs에 없는 로그 줄·인게임 연결/실제 화면을 검증해야 할 때 추가. API 기반 알고리즘 개선의 필수 설치 요건은 아님 |

권고하는 첫 실행 단위는 **기존 corpus를 고정 → 기존 HEAD와 후보 알고리즘을 같은 표본에서 평가 → 한 가지 일반적 결함만 수정 → 독립 report에서 회귀 확인**이다. cactbot 참조는 사람이 선택한 표현의 근거로 사용하고, 텍스트 일치를 유일한 성공 기준으로 삼지 않는다. 더 많은 로그가 모든 잘못된 선택을 해결하지는 않으므로, 표본 증가와 별개로 누락·잘못된 분기·예고 실패가 구분되는 판정이 필요하다.

## 현재 작업 공간 점검 — 2026-10-15

외부 조사와 별도로 현재 실행 환경과 소스를 확인했다. 알고리즘 수정, 새 FFLogs 수집, 의존성 설치는 실행하지 않았다.

| 영역 | 직접 확인한 상태 | 추가로 필요한 것 |
| --- | --- | --- |
| 실행 도구 | Rust stable/nightly, Cargo 1.99.0, Node 24.22.0, npm 11.19.1, Nushell 0.116.1, Python/uv, jq, curl, Git, ast-grep, sd 사용 가능 | 필수 추가 설치 없음 |
| 수집 인증 | 현재 프로세스의 `FFLOGS_CLIENT_ID`, `FFLOGS_CLIENT_SECRET` 모두 미설정; 값은 출력하지 않음 | 새 수집 전 실행 환경에 설정. 발급·유효성·실제 접근 권한은 미검증 |
| 기존 자료 | `logs/`에 280개 JSON, 약 1.7 GiB: Dancing Mad 56, Clyteum 21, R11S 44, R12S 159 | 기존 자료로 초기 평가 가능. 향후 서로 다른 보고서·진행도·처치 시간의 표본 확대 |
| FFLogs CLI | `src/fflogs/mod.rs`: 보고서 코드로 fight 목록, 이름 필터, 같은 encounter/difficulty의 완료된 pull 일괄 수집 | 보고서 후보 탐색과 수집 계획 기능 없음 |
| 수집 안정성 | `src/fflogs/client.rs`: 페이지 커서 검증, 수집 완결성/원본 요청 기록, 일시 오류 두 번 재시도, 최대 60초 Retry-After. `storage.rs`: fight 단위 원자적 저장 | API 사용량 조회·실행 예산·중복 건너뛰기·중단 후 재개를 연결할 작은 드라이버. 기존 파일은 교체될 수 있으므로 원본 스냅샷 보존 |
| cactbot | `/home/ks2515/workspace/cactbot` 존재. 로컬과 원격 HEAD 모두 `e79ea7f16093be5b80cda0e2d5bc8eb899d6fb4b`. TypeScript/ts-node/Mocha/Chai/jsdom/js-yaml 설치 확인 | 기준 커밋 고정. 원본 수정 대신 btimeline 또는 /tmp 안의 검증용 작업 공간 사용 |
| 소비자 실행 | 두 `verify_*.mjs`의 Node 문법 검사 통과. 실제 cactbot `timeline_parser.ts`, `timeline.ts` import 성공 | 전체 검증을 이번 조사에서 다시 돌리지는 않음 |
| 공식 검사 | 자체 소비자 검증과 별도로 cactbot의 `test/helper/test_timeline.ts`, `test/test_data_files.ts` 존재 | 생성 파일과 대응 trigger 파일을 공식 검사에 연결. 자체 하네스의 통과를 공식 검사 통과로 간주하지 않기 |
| 평가 분리 | `freeze_evaluation_inputs.nu`는 SHA-256과 중복 후보 그룹을 고정. 분리 기준은 candidate_group이며 일반적으로 파일 단위 | 독립 보고서 전체를 holdout으로 묶고 동일 전투의 중복 업로드도 분리 금지 |
| 평가 범위 | `evaluate_replay.nu`는 raid/dungeon만 받고 encounter별 생성. `verify_phase_schedule.mjs`는 실제 엔진의 예측 표시/누락·추가 sync 검사 | alliance 및 복합 checkpoint 평가 경로를 통합 러너에 연결 |
| 참조 비교 | `agent/timeline-quality.md`에 수동 cactbot 비교 기록. `audit_timeline_sequences.py`는 원본 로그 간 비교 | cactbot 참조 행·분기·반복·의도적 제외와 생성 결과를 정렬해 차이를 산출하는 비교기 |
| 플러그인 | FFLogs 검색 결과는 0건. 직접 API 수집기와 GitHub 도구는 이미 사용 가능 | 이 검색 범위에서 필수 새 MCP/플러그인은 확인되지 않음 |

소스 근거: [FFLogs CLI](../src/fflogs/mod.rs), [FFLogs 클라이언트](../src/fflogs/client.rs), [저장](../src/fflogs/storage.rs), [입력 고정](../scripts/freeze_evaluation_inputs.nu), [재현 평가](../scripts/evaluate_replay.nu), [원본 로그 감사](../scripts/audit_timeline_sequences.py), [소비자 검증](../scripts/verify_cactbot.mjs), [예측 표시 검증](../scripts/verify_phase_schedule.mjs), [기존 품질 기록](timeline-quality.md).

실행 우선순위는 기존 280개 로그의 독립 평가와 참조 비교부터다. 새 수집은 인증 환경을 준비한 뒤 실패 사례에 필요한 표본을 보충하는 순서로 진행할 수 있다.

예측 품질의 분모를 생성된 행에만 두면 통째로 빠진 기믹이 점수에서 사라진다. 원본 cast 및 독립 cactbot 참조를 함께 확인하고, 의도적 제외가 늘어 누락률만 낮아지는 경우도 개선으로 취급하지 않는다.

## 조사 한계

외부 수집, 인증, 패키지 설치, 생성기 변경은 하지 않았다. 위 수집 드라이버와 평가 통합은 필요한 기능의 제안이며 구현·실행 완료 보고가 아니다. 실제 비용·API 접근 가능 여부와 raw ACT 수준의 완전성은 아직 검증되지 않았다.

## 후속 도구 구현 — 2026-10-15

위 내용은 구현 전 조사 시점의 기록이다. 이후 사용자 요청으로 도구를 구현했고, 사용법은 [README](../README.md)에 정리했다.

| 항목 | 후속 확인 |
| --- | --- |
| 인증 | 수정된 `.env`로 토큰 발급과 실제 API 조회 성공. 비밀 값은 출력하지 않음 |
| 수집/다운로드 | `fflogs-collect`로 후보 계획 저장, `fflogs-download`로 1개 전투의 9,471개 이벤트 다운로드. 재실행 시 다운로드 0/검증 후 건너뜀 1 확인. 기존 `fflogs` 유지 |
| 비교/공식 검사 | 공식 파서 기반 비교기와 원본 공식 검사 호출기 추가. R11S 참조 파일의 공식 검사 통과 |
| 평가 | 보고서 단위 split, alliance/복합 encounter 경로, 원본 cast 감사·공식 검사·실제 소비자·참조 비교를 묶는 실행기 추가. 의도적으로 맞지 않는 참조에서 전체 실패 전파 확인 |
| 범위 | 생성 알고리즘은 변경하지 않음. 실제 ACT·브라우저 표시 및 전체 corpus의 새 평가 결과는 이 도구 구현만으로 확정하지 않음 |
