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
        partition: None,
        region: None,
        page: 1,
        pages: 2,
        max_reports: 10,
        max_fights: 100,
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
    let fights = candidate_fights(&report, None, None).unwrap();
    (
        DownloadPlan {
            schema_version: 1,
            collected_at_unix_ms: 1,
            source: PlanSource {
                reports: vec!["synthetic".into()],
                encounter: None,
                difficulty: None,
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
    assert!(plan.validate().is_err());
}
