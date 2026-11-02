use rstest::rstest;
use wiremock::ResponseTemplate;
use wiremock::matchers::body_partial_json;

use super::*;
use crate::fflogs::tests::{api, authenticate, metadata, report, server, token};

fn quota(spent: f64) -> Value {
    json!({"limitPerHour":3600,"pointsSpentThisHour":spent,"pointsResetIn":3000})
}

async fn mount_quota(server: &wiremock::MockServer) {
    api("TimelineQuota")
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"data":{"rateLimitData":quota(1.0)}})),
        )
        .expect(1)
        .mount(server)
        .await;
}

fn options() -> FFLogsCollectCommand {
    FFLogsCollectCommand {
        reports: vec![],
        encounter: Some(105),
        difficulty: Some(101),
        game_zone: None,
        partition: None,
        region: None,
        page: 1,
        pages: 2,
        max_reports: 10,
        max_fights: 100,
        purpose: "training".into(),
        output: "unused.json".into(),
        access: Access {
            client_id: "id".into(),
            client_secret: "secret".into(),
            max_requests: 100,
            max_points: 1000.0,
        },
    }
}

#[rstest]
#[case(false)]
#[case(true)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn discovery_deduplicates_reports_and_never_downloads_events(#[case] more: bool) {
    let server = server(token(), None).await;
    mount_quota(&server).await;
    api("TimelineRankings").and(body_partial_json(json!({"variables":{"page":1}})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data":{
            "rateLimitData":quota(2.0), "worldData":{"encounter":{"fightRankings":{
                "rankings":[{"report":{"code":"example"}},{"report":{"code":"example"}}],"hasMorePages":more
            }}}}}))).expect(1).mount(&server).await;
    let mut data = metadata().1;
    data["data"]["rateLimitData"] = quota(3.0);
    let uploading = json!({"id":2,"encounterID":105,"difficulty":101,"inProgress":true});
    data["data"]["reportData"]["report"]["fights"]
        .as_array_mut()
        .unwrap()
        .push(uploading);
    data["data"]["reportData"]["report"]["fights"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id":3,"encounterID":0,"difficulty":null,"inProgress":false}));
    api("TimelineCandidates")
        .respond_with(ResponseTemplate::new(200).set_body_json(data))
        .expect(1)
        .mount(&server)
        .await;
    api("TimelineEvents")
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&server)
        .await;
    let client = authenticate(&server).with_budget(10, 100.0).unwrap();
    let mut command = options();
    command.pages = 1;
    let plan = command.collect(&client).unwrap();
    plan.validate().unwrap();
    assert!(plan.failures.is_empty());
    assert_eq!(plan.fights.len(), 1);
    assert_eq!(plan.fights[0].report, "example");
    assert_eq!(plan.truncated, more);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn budget_exhaustion_retains_partial_discovery_and_counts_retries() {
    let server = server(token(), None).await;
    mount_quota(&server).await;
    api("TimelineCandidates")
        .respond_with(ResponseTemplate::new(503))
        .expect(1)
        .mount(&server)
        .await;
    let client = authenticate(&server).with_budget(2, 100.0).unwrap();
    let mut command = options();
    command.reports = vec!["example".into(), "later".into()];
    let plan = command.collect(&client).unwrap();
    assert_eq!(plan.failures.len(), 1);
    assert!(plan.failures[0].error.contains("budget exhausted"));
    assert!(plan.fights.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn observed_point_limit_stops_before_another_request() {
    let server = server(token(), None).await;
    mount_quota(&server).await;
    let mut body = metadata().1;
    body["data"]["rateLimitData"] = quota(5.0);
    api("TimelineCandidates")
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .expect(1)
        .mount(&server)
        .await;
    let client = authenticate(&server).with_budget(10, 4.0).unwrap();
    let mut command = options();
    command.reports = vec!["example".into(), "later".into()];
    let plan = command.collect(&client).unwrap();
    assert_eq!(plan.fights.len(), 1);
    assert_eq!(plan.failures.len(), 1);
    assert!(plan.failures[0].error.contains("point budget"));
}

fn fixture_plan() -> (DownloadPlan, Value) {
    let data: Value =
        serde_json::from_str(include_str!("../../../tests/fixtures/pull.json")).unwrap();
    let mut report = data["report"].clone();
    report["archiveStatus"] = json!({"isAccessible":true});
    let fights = candidate_fights(&report, None, None, None).unwrap();
    (
        DownloadPlan {
            schema_version: 3,
            purpose: Some(Purpose::Training),
            selection_policy: Purpose::Training.policy().into(),
            candidate_count: fights.len(),
            collected_at_unix_ms: 1,
            source: PlanSource {
                reports: vec!["synthetic".into()],
                encounter: None,
                difficulty: None,
                game_zone: None,
                partition: None,
                region: None,
                first_page: 1,
                pages: 1,
            },
            fights,
            failures: vec![],
            truncated: false,
        },
        data,
    )
}

#[test]
fn completed_downloads_resume_without_auth_and_invalid_files_are_preserved() {
    let (plan, mut data) = fixture_plan();
    let directory = tempfile::tempdir().unwrap();
    let root = Utf8Path::from_path(directory.path()).unwrap();
    let path = root.join("synthetic_1.json");
    fs::write(&path, serde_json::to_vec(&data).unwrap()).unwrap();
    let result = download(&plan, root, 1, 1000, || {
        panic!("completed file must skip authentication")
    })
    .unwrap();
    assert_eq!(result.skipped, 1);
    assert_eq!(result.downloaded, 0);
    data["collection"]["complete"] = json!(false);
    let invalid = serde_json::to_vec(&data).unwrap();
    fs::write(&path, &invalid).unwrap();
    let failed = download(&plan, root, 1, 1000, || {
        panic!("invalid file must not be overwritten")
    })
    .unwrap();
    assert_eq!(failed.failures.len(), 1);
    assert_eq!(fs::read(&path).unwrap(), invalid);
    assert_eq!(failed.remaining, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn download_enforces_byte_limit_then_resumes_after_atomic_save() {
    let (plan, data) = fixture_plan();
    let directory = tempfile::tempdir().unwrap();
    let root = Utf8Path::from_path(directory.path()).unwrap();
    let mut metadata = data["report"].clone();
    metadata["archiveStatus"] = json!({"isAccessible":true});
    for max_bytes in [1, u64::MAX] {
        let server = server(token(), Some(report(metadata.clone()))).await;
        api("TimelineEvents")
            .respond_with(ResponseTemplate::new(200).set_body_json(
                report(json!({"events":{"data":data["events"],"nextPageTimestamp":null}})).1,
            ))
            .expect(1)
            .mount(&server)
            .await;
        let result = download(&plan, root, 10, max_bytes, || Ok(authenticate(&server))).unwrap();
        if max_bytes == 1 {
            assert_eq!(result.failures.len(), 1);
            assert!(result.failures[0].error.contains("byte budget"));
            assert!(!root.join("synthetic_1.json").exists());
        } else {
            assert!(result.failures.is_empty());
            assert_eq!(result.downloaded, 1);
        }
    }
    let resumed = download(&plan, root, 10, 1, || {
        panic!("saved file must not be downloaded again")
    })
    .unwrap();
    assert_eq!(resumed.skipped, 1);
}

#[test]
fn plans_reject_duplicate_or_unsafe_identities() {
    let (mut plan, _) = fixture_plan();
    plan.validate().unwrap();
    plan.fights[0].report = "../outside".into();
    assert!(plan.validate().is_err());
    plan.fights[0].report = "synthetic".into();
    plan.fights
        .push(serde_json::from_value(serde_json::to_value(&plan.fights[0]).unwrap()).unwrap());
    plan.candidate_count = plan.fights.len();
    assert!(plan.validate().is_err());
}

#[rstest]
#[case(json!({"id":1307,"name":"Hell on Rails"}), 1)]
#[case(json!({"id":1308,"name":"Hell on Rails (Extreme)"}), 0)]
#[case(Value::Null, 0)]
fn game_zone_filter_distinguishes_normal_from_extreme_with_the_same_difficulty(
    #[case] zone: Value,
    #[case] expected: usize,
) {
    let mut data = metadata().1["data"]["reportData"]["report"].clone();
    data["fights"][0]["gameZone"] = zone;
    data["fights"][0]["difficulty"] = 100.into();
    assert_eq!(
        candidate_fights(&data, None, Some(100), Some(1307))
            .unwrap()
            .len(),
        expected
    );
}

#[rstest]
#[case("combatTime", json!(-1))]
#[case("combatTime", json!(100000))]
#[case("fightPercentage", json!(101))]
#[case("bossPercentage", json!(-1))]
#[case("lastPhaseAsAbsoluteIndex", json!(-1))]
#[case("gameZone", json!({"id":0,"name":"Invalid"}))]
fn rejects_invalid_candidate_metadata(#[case] field: &str, #[case] value: Value) {
    let mut data = metadata().1["data"]["reportData"]["report"].clone();
    data["fights"][0][field] = value;
    candidate_fights(&data, None, None, None).unwrap_err();
}

#[test]
fn plan_v3_is_explicit_and_old_plans_are_rejected() {
    let (mut plan, _) = fixture_plan();
    insta::assert_snapshot!(serde_json::to_string_pretty(&plan).unwrap());
    for version in [1, 2] {
        plan.schema_version = version;
        let mut legacy = serde_json::to_value(&plan).unwrap();
        legacy.as_object_mut().unwrap().remove("purpose");
        let legacy: DownloadPlan = serde_json::from_value(legacy).unwrap();
        assert!(
            legacy
                .validate()
                .unwrap_err()
                .to_string()
                .contains("regenerate with fflogs-collect")
        );
    }
    plan.schema_version = 3;
    plan.purpose = None;
    plan.validate().unwrap_err();
    plan.purpose = Some(Purpose::Verification);
    plan.validate().unwrap_err();
    plan.selection_policy = Purpose::Verification.policy().into();
    plan.validate().unwrap();
    plan.source.game_zone = Some(1307);
    plan.validate().unwrap_err();
}

fn candidate_set() -> Vec<PlannedFight> {
    let mut candidates = Vec::new();
    for report in ["alpha", "beta"] {
        for (fight, kill, duration) in [
            (1, true, 900),
            (2, true, 500),
            (3, false, 800),
            (4, false, 400),
            (5, true, 100),
        ] {
            let mut row: PlannedFight = serde_json::from_value(json!({"report":report,"fight":fight,"name":"Boss","encounter":1,"difficulty":100,"kill":kill,"startTime":0,"endTime":duration})).unwrap();
            row.metadata.game_zone = Some(selection::GameZone {
                id: 1307,
                name: "Normal".into(),
            });
            candidates.push(row);
        }
    }
    candidates
}

#[test]
fn training_prioritizes_long_progress_without_short_or_outcome_quotas() {
    let selected = prioritize(candidate_set(), 8, Purpose::Training);
    assert_eq!(
        selected
            .iter()
            .map(|fight| (fight.report.as_str(), fight.fight))
            .collect::<Vec<_>>(),
        [
            ("alpha", 1),
            ("beta", 1),
            ("alpha", 3),
            ("beta", 3),
            ("alpha", 2),
            ("beta", 2),
            ("alpha", 4),
            ("beta", 4),
        ]
    );
}

#[test]
fn long_wipes_precede_short_clears_and_use_combat_time_when_available() {
    let mut candidates = candidate_set();
    for fight in &mut candidates {
        if fight.kill {
            fight.metadata.combat_time = Some(50);
        }
    }
    let selected = prioritize(candidates, 2, Purpose::Training);
    assert!(selected.iter().all(|fight| !fight.kill && fight.fight == 3));
}

#[test]
fn verification_retains_independent_reports_outcomes_and_short_controls() {
    let selected = prioritize(candidate_set(), 8, Purpose::Verification);
    assert_eq!(
        selected
            .iter()
            .take(2)
            .map(|fight| (fight.report.as_str(), fight.fight))
            .collect::<Vec<_>>(),
        [("alpha", 1), ("beta", 3)]
    );
    assert!(
        selected
            .iter()
            .take(4)
            .all(|fight| [1, 3].contains(&fight.fight))
    );
    assert!(
        selected
            .iter()
            .skip(4)
            .all(|fight| [4, 5].contains(&fight.fight))
    );
    assert_eq!(
        selected
            .iter()
            .filter(|fight| fight.report == "alpha")
            .count(),
        4
    );
}

proptest::proptest! {
    #[test]
    fn prioritization_is_order_independent_and_never_fabricates_fights(lengths in proptest::collection::vec(1_i64..10000, 1..40), limit in 0_usize..50, training in proptest::bool::ANY) {
        let candidates = |reverse: bool| {
            let mut rows: Vec<PlannedFight> = lengths.iter().enumerate().map(|(index,duration)| serde_json::from_value(json!({"report":if index%2==0 {"alpha"}else{"beta"},"fight":index+1,"name":"Boss","encounter":1,"difficulty":100,"kill":index%3!=0,"startTime":0,"endTime":duration})).unwrap()).collect();
            if reverse {rows.reverse();} rows
        };
        let identities = |rows: Vec<PlannedFight>| rows.into_iter().map(|fight| (fight.report,fight.fight)).collect::<Vec<_>>();
        let purpose = if training { Purpose::Training } else { Purpose::Verification };
        let selected = identities(prioritize(candidates(false),limit,purpose));
        proptest::prop_assert_eq!(selected.len(),limit.min(lengths.len()));
        proptest::prop_assert_eq!(&selected,&identities(prioritize(candidates(true),limit,purpose)));
        proptest::prop_assert_eq!(selected.iter().collect::<BTreeSet<_>>().len(),selected.len());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn game_zone_discovery_paginates_and_checks_direct_report_metadata() {
    let server = server(token(), None).await;
    mount_quota(&server).await;
    for (page, more) in [(1, true), (2, false)] {
        api("TimelineZoneReports").and(body_partial_json(json!({"variables":{"zone":1307,"page":page}})))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data":{"rateLimitData":quota(2.0),"reportData":{"reports":{"data":[{"code":"example"}],"has_more_pages":more}}}})))
            .expect(1).mount(&server).await;
    }
    let mut data = metadata().1;
    data["data"]["reportData"]["report"]["fights"][0]["gameZone"] =
        json!({"id":1307,"name":"Hell on Rails"});
    data["data"]["rateLimitData"] = quota(3.0);
    api("TimelineCandidates")
        .respond_with(ResponseTemplate::new(200).set_body_json(data))
        .expect(1)
        .mount(&server)
        .await;
    let mut command = options();
    command.game_zone = Some(1307);
    let plan = command
        .collect(&authenticate(&server).with_budget(10, 100.0).unwrap())
        .unwrap();
    plan.validate().unwrap();
    assert_eq!(plan.candidate_count, 1);
    assert_eq!(plan.fights[0].metadata.zone_id(), Some(1307));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn changed_zone_is_rejected_before_fetching_any_events() {
    let (mut plan, mut data) = fixture_plan();
    plan.fights[0].metadata.game_zone = Some(selection::GameZone {
        id: 1307,
        name: "Normal".into(),
    });
    data["report"]["fights"][0]["gameZone"] = json!({"id":1308,"name":"Extreme"});
    data["report"]["archiveStatus"] = json!({"isAccessible":true});
    let server = server(token(), Some(report(data["report"].clone()))).await;
    api("TimelineEvents")
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&server)
        .await;
    let directory = tempfile::tempdir().unwrap();
    let result = download(
        &plan,
        Utf8Path::from_path(directory.path()).unwrap(),
        1,
        u64::MAX,
        || Ok(authenticate(&server)),
    )
    .unwrap();
    assert_eq!(result.downloaded, 0);
    assert!(result.failures[0].error.contains("metadata differs"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fight_limit_is_applied_after_inspecting_all_report_candidates() {
    let server = server(token(), None).await;
    mount_quota(&server).await;
    for (code, end) in [("alpha", 30), ("beta", 90)] {
        let mut data = metadata().1;
        data["data"]["rateLimitData"] = quota(2.0);
        data["data"]["reportData"]["report"]["code"] = code.into();
        data["data"]["reportData"]["report"]["fights"][0]["endTime"] = end.into();
        api("TimelineCandidates")
            .and(body_partial_json(json!({"variables":{"code":code}})))
            .respond_with(ResponseTemplate::new(200).set_body_json(data))
            .expect(1)
            .mount(&server)
            .await;
    }
    let mut command = options();
    command.reports = vec!["alpha".into(), "beta".into()];
    command.max_fights = 1;
    let plan = command
        .collect(&authenticate(&server).with_budget(10, 100.0).unwrap())
        .unwrap();
    assert_eq!(plan.candidate_count, 2);
    assert_eq!(plan.fights[0].report, "beta");
    assert!(plan.truncated);
    plan.validate().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn verification_plan_records_its_purpose_and_selection_policy() {
    let server = server(token(), None).await;
    let mut data = metadata().1;
    data["data"]["rateLimitData"] = quota(2.0);
    api("TimelineCandidates")
        .respond_with(ResponseTemplate::new(200).set_body_json(data))
        .expect(1)
        .mount(&server)
        .await;
    let mut command = options();
    command.reports = vec!["example".into()];
    command.purpose = "verification".into();
    let plan = command.collect(&authenticate(&server)).unwrap();
    plan.validate().unwrap();
    assert_eq!(plan.purpose, Some(Purpose::Verification));
    assert_eq!(plan.selection_policy, Purpose::Verification.policy());
}
