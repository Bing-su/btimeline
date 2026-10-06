#![allow(
    clippy::unwrap_used,
    reason = "test fixtures use unwrap to fail at the source"
)]

use std::fs;

use rstest::{fixture, rstest};
use serde_json::json;
use wiremock::matchers::{body_partial_json, body_string_contains, header, method, path};
use wiremock::{Mock, MockBuilder, MockServer, ResponseTemplate};

use super::*;

fn token() -> (u16, Value) {
    (
        200,
        json!({"access_token":"test-token", "token_type":"Bearer"}),
    )
}

fn report(value: Value) -> (u16, Value) {
    (200, json!({"data":{"reportData":{"report":value}}}))
}

fn metadata() -> (u16, Value) {
    report(
        json!({"code":"example","revision":1,"startTime":0,"endTime":100,"archiveStatus":{"isAccessible":true},"phases":null,
        "masterData":{"lang":"en","gameVersion":1,"logVersion":76,"actors":[],"abilities":[]},
        "fights":[{"id":1,"name":"Lindwurm II","encounterID":105,"difficulty":101,"kill":true,
            "fightPercentage":100,"bossPercentage":0,"lastPhase":0,"enemyNPCs":[],"enemyPets":[],
            "startTime":10,"endTime":30,"inProgress":false,"phaseTransitions":null}]}),
    )
}

fn response((status, body): (u16, Value)) -> ResponseTemplate {
    ResponseTemplate::new(status).set_body_json(body)
}

// Match real paths, authorization and query kinds instead of reimplementing an HTTP server.
fn api(query: &str) -> MockBuilder {
    Mock::given(method("POST"))
        .and(path("/api/v2/client"))
        .and(header("Authorization", "Bearer test-token"))
        .and(body_string_contains(query))
}

async fn server(
    token_response: (u16, Value),
    metadata_response: Option<(u16, Value)>,
) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .and(header("Authorization", "Basic aWQ6c2VjcmV0"))
        .and(body_string_contains("grant_type=client_credentials"))
        .respond_with(response(token_response))
        .expect(1)
        .mount(&server)
        .await;
    if let Some(metadata) = metadata_response {
        api("TimelineMetadata")
            .respond_with(response(metadata))
            .expect(1)
            .mount(&server)
            .await;
    }
    server
}

fn authenticate(server: &MockServer) -> Client {
    Client::authenticate(
        &format!("{}/oauth/token", server.uri()),
        &format!("{}/api/v2/client", server.uri()),
        "id",
        "secret",
    )
    .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn collects_two_pages_and_saves_raw_json() {
    let server = server(token(), Some(metadata())).await;
    api("TimelineEvents")
        .and(body_partial_json(
            json!({"variables":{"start":10.0,"fightIDs":[1]}}),
        ))
        .respond_with(response(report(
            json!({"events":{"data":[{"type":"cast","timestamp":11,"optional":{"x":1}}],"nextPageTimestamp":20}}),
        )))
        .expect(1)
        .mount(&server)
        .await;
    api("TimelineEvents")
        .and(body_partial_json(json!({"variables":{"start":20.0}})))
        .respond_with(response(report(
            json!({"events":{"data":[{"type":"cast","timestamp":21}],"nextPageTimestamp":null}}),
        )))
        .expect(1)
        .mount(&server)
        .await;
    let data = authenticate(&server).collect("example", 1).unwrap();
    assert_eq!(data["events"].as_array().unwrap().len(), 2);
    assert_eq!(data["events"][0]["optional"]["x"], 1);
    assert!(data["report"]["phases"].is_null());
    assert_eq!(data["report"]["fights"][0]["fightPercentage"], 100);
    assert_eq!(data["report"]["fights"][0]["bossPercentage"], 0);
    assert_eq!(data["collection"]["pageCount"], 2);
    assert_eq!(data["collection"]["eventCount"], 2);
    assert_eq!(data["collection"]["pageStartTimes"], json!([10.0, 20.0]));
    assert_eq!(data["collection"]["complete"], true);
    assert!(data["collection"]["nextPageTimestamp"].is_null());
    assert_eq!(
        data["collection"]["requests"]["metadata"]["variables"]["fightIDs"],
        json!([1])
    );
    assert!(data["collection"]["collectedAtUnixMs"].as_u64().is_some());
    let directory = tempfile::tempdir().unwrap();
    let dir = directory.path();
    save(&data, dir, "example_1.json", OutputFormat::Json).unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(dir.join("example_1.json")).unwrap()).unwrap(),
        data
    );
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 4);
    let metadata: Value = requests[1].body_json().unwrap();
    let events: Value = requests[2].body_json().unwrap();
    assert!(
        metadata["query"]
            .as_str()
            .unwrap()
            .contains("fightPercentage bossPercentage")
    );
    assert!(
        events["query"]
            .as_str()
            .unwrap()
            .contains("includeResources: true")
    );
    server.verify().await;
}

#[rstest]
#[case::unauthorized(401, json!({}))]
#[case::empty_token(200, json!({"access_token":"", "token_type":"Bearer"}))]
#[case::unsupported_type(200, json!({"access_token":"test", "token_type":"Other"}))]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rejects_bad_authentication(#[case] status: u16, #[case] body: Value) {
    let token_response = (status, body);
    let server = server(token_response, None).await;
    assert!(
        Client::authenticate(
            &format!("{}/oauth/token", server.uri()),
            &format!("{}/api/v2/client", server.uri()),
            "id",
            "secret"
        )
        .is_err()
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
    server.verify().await;
}

#[rstest]
#[case::missing_report("/data/reportData/report", Value::Null)]
#[case::inaccessible_archive("/data/reportData/report/archiveStatus/isAccessible", json!(false))]
#[case::uploading_fight("/data/reportData/report/fights/0/inProgress", json!(true))]
#[case::missing_fight("/data/reportData/report/fights", json!([]))]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rejects_missing_or_unavailable_fights(#[case] pointer: &str, #[case] value: Value) {
    // Modify one field per case so uploading still has accessible archive metadata.
    let mut metadata_response = metadata();
    *metadata_response.1.pointer_mut(pointer).unwrap() = value;
    let server = server(token(), Some(metadata_response)).await;
    authenticate(&server).collect("example", 1).unwrap_err();
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
    server.verify().await;
}

#[rstest]
#[case::stationary_cursor("stationary", report(json!({"events":{"data":[],"nextPageTimestamp":10}})), 1)]
#[case::backward_cursor("backward", report(json!({"events":{"data":[],"nextPageTimestamp":9}})), 1)]
#[case::graphql_error("graphql", (200, json!({"errors":[{"message":"denied"}]})), 1)]
#[case::invalid_events("invalid", report(json!({"events":{"data":{},"nextPageTimestamp":null}})), 1)]
#[case::unavailable_service("unavailable", (503, json!({})), 3)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failures_preserve_previous_output(
    #[case] name: &str,
    #[case] page: (u16, Value),
    #[case] expected: u64,
) {
    // Isolate parallel cases, e.g. 503 retries cannot overwrite a cursor case's output.
    let directory = tempfile::tempdir().unwrap();
    let dir = directory.path();
    let target = dir.join("example_1.json");
    fs::write(&target, "previous").unwrap();

    let server = server(token(), Some(metadata())).await;

    api("TimelineEvents")
        .respond_with(response(page))
        .expect(expected)
        .mount(&server)
        .await;
    let result = authenticate(&server)
        .collect("example", 1)
        .and_then(|data| save(&data, dir, "example_1.json", OutputFormat::Json));
    assert!(result.is_err(), "{name} must fail before replacing output");
    assert_eq!(fs::read_to_string(&target).unwrap(), "previous");
    server.verify().await;
    assert_eq!(fs::read_dir(dir).unwrap().count(), 1);
}

#[test]
fn batch_name_matching_is_case_insensitive_and_preserves_sorted_unique_ids() {
    // Match one named pull, then include its whole group, e.g. a differently named wipe.
    let data = json!({"fights": [
        {"id":3,"name":"Lindwurm II","encounterID":105,"difficulty":101,"inProgress":false},
        {"id":1,"name":"Lindwurm","encounterID":105,"difficulty":101,"inProgress":false},
        {"id":3,"name":"Lindwurm II","encounterID":105,"difficulty":101,"inProgress":false},
        {"id":2,"name":"Lindwurm II","encounterID":105,"difficulty":101,"inProgress":true},
        {"id":4,"name":"Other","encounterID":104,"difficulty":101,"inProgress":false}
    ]});
    assert_eq!(
        select_fights(&data, None, Some("LINDWURM II"), true).unwrap(),
        vec![1, 3]
    );
    assert_eq!(select_fights(&data, Some(4), None, false).unwrap(), vec![4]);
    select_fights(&data, Some(99), None, false).unwrap_err();
    let mut listing = Vec::new();
    let listing_data = json!({"fights": [
        {"id":1,"name":"Lindwurm II","startTime":0,"endTime":1000},
        {"id":2,"name":"Other","startTime":0,"endTime":1000}
    ]});
    list_fights(&listing_data, Some("LINDWURM"), &mut listing).unwrap();
    let listing = String::from_utf8(listing).unwrap();
    assert!(listing.contains("Lindwurm II"));
    assert!(!listing.contains("Other"));
}

// Mixed phases must not silently join; wipes and different names stay in the same group.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lists_and_collects_only_the_selected_batch_with_one_metadata_request() {
    let mut data = metadata().1["data"]["reportData"]["report"].clone();
    let kill = data["fights"][0].clone();
    let mut wipe = kill.clone();
    wipe["id"] = json!(2);
    wipe["kill"] = json!(false);
    wipe["name"] = json!("Lindwurm");
    wipe["fightPercentage"] = json!(70);
    let mut other_phase = kill.clone();
    other_phase["id"] = json!(3);
    other_phase["encounterID"] = json!(104);
    let mut uploading = kill.clone();
    uploading["id"] = json!(4);
    uploading["inProgress"] = json!(true);
    let mut other_difficulty = kill.clone();
    other_difficulty["id"] = json!(5);
    other_difficulty["difficulty"] = json!(100);
    data["fights"] = json!([kill, wipe, other_phase, uploading, other_difficulty]);
    select_fights(&data, None, None, true).unwrap_err();
    select_fights(&data, None, Some("Lindwurm"), true).unwrap_err();
    select_fights(&data, Some(99), None, true).unwrap_err();
    assert_eq!(
        select_fights(&data, Some(1), None, true).unwrap(),
        vec![1, 2]
    );
    let mut listing = Vec::new();
    list_fights(&data, Some("lindwurm"), &mut listing).unwrap();
    let listing = String::from_utf8(listing).unwrap();
    assert!(listing.contains("wipe") && listing.contains("uploading") && listing.contains("70"));
    let server = server(token(), Some(report(data))).await;
    for id in [1, 2] {
        api("TimelineEvents")
            .and(body_partial_json(json!({"variables":{"fightIDs":[id]}})))
            .respond_with(response(report(
                json!({"events":{"data":[{"type":"cast","timestamp":10 + id}],"nextPageTimestamp":null}}),
            )))
            .expect(1)
            .mount(&server)
            .await;
    }
    let client = authenticate(&server);
    let metadata = client.metadata("example", None).unwrap();
    for id in select_fights(&metadata, Some(1), None, true).unwrap() {
        let result = serde_json::to_value(
            client
                .collect_from_report("example", id, metadata.clone(), None)
                .unwrap(),
        )
        .unwrap();
        assert_eq!(result["report"]["fights"].as_array().unwrap().len(), 1);
        assert_eq!(result["collection"]["fightID"], id);
        assert!(result["collection"]["requests"]["metadata"]["variables"]["fightIDs"].is_null());
        if id == 2 {
            assert_eq!(result["report"]["fights"][0]["kill"], false);
        }
    }
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 4);
    assert!(requests[1].body_json::<Value>().unwrap()["variables"]["fightIDs"].is_null());
    server.verify().await;
}

#[rstest]
#[case::rate_limited(429)]
#[case::bad_gateway(502)]
#[case::service_unavailable(503)]
#[case::gateway_timeout(504)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retries_transient_responses_but_not_permission_errors(#[case] status: u16) {
    let server = server(token(), None).await;
    api("TimelineMetadata")
        .respond_with(ResponseTemplate::new(status).insert_header("Retry-After", "0"))
        .up_to_n_times(1)
        .with_priority(1)
        .expect(1)
        .mount(&server)
        .await;
    api("TimelineMetadata")
        .respond_with(response(metadata()))
        .with_priority(2)
        .expect(1)
        .mount(&server)
        .await;
    authenticate(&server).metadata("example", None).unwrap();
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests[1].body, requests[2].body);
    server.verify().await;
    let denied = self::server(token(), Some((403, json!({})))).await;
    assert!(
        authenticate(&denied)
            .metadata("example", None)
            .unwrap_err()
            .to_string()
            .contains("403")
    );
    assert_eq!(denied.received_requests().await.unwrap().len(), 2);
    denied.verify().await;
}

#[rstest]
#[case::too_long("61")]
#[case::invalid("invalid")]
#[case::http_date("Wed, 21 Oct 2015 07:28:00 GMT")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rejects_unsupported_retry_after_without_retrying(#[case] retry_after: &str) {
    let server = server(token(), None).await;
    api("TimelineMetadata")
        .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", retry_after))
        .expect(1)
        .mount(&server)
        .await;
    assert!(
        authenticate(&server)
            .metadata("example", None)
            .unwrap_err()
            .to_string()
            .contains("Retry-After")
    );
    server.verify().await;
}

#[test]
fn atomic_save_replaces_existing_json_and_cleans_failed_temporaries() {
    let directory = tempfile::tempdir().unwrap();
    let dir = directory.path();
    fs::write(dir.join("pull.json"), "previous").unwrap();
    save(
        &json!({"complete":true}),
        dir,
        "pull.json",
        OutputFormat::JsonPretty,
    )
    .unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(dir.join("pull.json")).unwrap()).unwrap(),
        json!({"complete":true})
    );
    // Reject a directory target without leaving temporary files or altering the saved pull.
    fs::create_dir(dir.join("blocked.json")).unwrap();
    assert!(save(&json!({}), dir, "blocked.json", OutputFormat::Json).is_err());
    assert_eq!(fs::read_dir(dir).unwrap().count(), 2);
}

#[rstest]
#[case::single_fight(vec!["4"], Some(4), false, false)]
#[case::listing(vec!["--list"], None, true, false)]
#[case::batch_seed(vec!["4", "--all"], Some(4), false, true)]
#[case::batch_name(vec!["--all", "--name", "Lindwurm II"], None, false, true)]
fn cli_preserves_single_fight_and_supports_listing_and_batch_modes(
    #[case] options: Vec<&str>,
    #[case] fight_id: Option<i64>,
    #[case] list: bool,
    #[case] all: bool,
) {
    let mut args = vec!["fflogs", "example", "-i", "test-id", "-s", "test-secret"];
    args.extend(options);
    let args: Vec<_> = args.into_iter().map(std::ffi::OsStr::new).collect();
    let parsed = crate::cli::MainCli::parse_from(&args).unwrap();
    let crate::cli::MainCommands::Fflogs(command) = parsed.command else {
        panic!("expected fflogs command")
    };
    command.validate().unwrap();
    assert_eq!(
        (command.fight_id, command.list, command.all),
        (fight_id, list, all)
    );
}

// Start every validation case from a valid command to prevent failures masking each other.
#[fixture]
fn command() -> FFLogsCommand {
    FFLogsCommand {
        report_code: "example".into(),
        fight_id: Some(1),
        list: false,
        all: false,
        name: None,
        client_id: "id".into(),
        client_secret: "secret".into(),
        output: None,
        format: OutputFormat::Json,
    }
}

#[rstest]
#[case::valid(|_: &mut FFLogsCommand| {}, true)]
#[case::list_with_fight(|c: &mut FFLogsCommand| c.list = true, false)]
#[case::list_with_all(|c: &mut FFLogsCommand| { c.fight_id = None; c.list = true; c.all = true; }, false)]
#[case::name_without_mode(|c: &mut FFLogsCommand| c.name = Some("Lindwurm".into()), false)]
#[case::empty_name(|c: &mut FFLogsCommand| { c.all = true; c.name = Some(" ".into()); }, false)]
#[case::invalid_report(|c: &mut FFLogsCommand| c.report_code = "bad/code".into(), false)]
#[case::invalid_fight(|c: &mut FFLogsCommand| c.fight_id = Some(0), false)]
#[case::missing_fight(|c: &mut FFLogsCommand| c.fight_id = None, false)]
#[case::empty_client_id(|c: &mut FFLogsCommand| c.client_id.clear(), false)]
#[case::empty_client_secret(|c: &mut FFLogsCommand| c.client_secret.clear(), false)]
fn command_validation_rejects_invalid_options_before_network_access(
    mut command: FFLogsCommand,
    #[case] change: fn(&mut FFLogsCommand),
    #[case] valid: bool,
) {
    change(&mut command);
    assert_eq!(command.validate().is_ok(), valid);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn isolates_reports_with_the_same_fight_id_and_absolute_pull_times() {
    // Two uploads can share a pull's absolute time; keep each report's raw clock and pages.
    let server = server(token(), None).await;
    for (code, origin, start) in [("reportA", 1000, 10), ("reportB", 900, 110)] {
        let mut data = metadata().1["data"]["reportData"]["report"].clone();
        data["code"] = json!(code);
        data["startTime"] = json!(origin);
        data["fights"][0]["startTime"] = json!(start);
        data["fights"][0]["endTime"] = json!(start + 20);
        api("TimelineMetadata")
            .and(body_partial_json(
                json!({"variables":{"code":code,"fightIDs":[1]}}),
            ))
            .respond_with(response(report(data)))
            .expect(1)
            .mount(&server)
            .await;
        for (cursor, timestamp, next) in [
            (start, start + 1, json!(start + 10)),
            (start + 10, start + 11, Value::Null),
        ] {
            api("TimelineEvents")
                .and(body_partial_json(json!({"variables":{
                    "code":code,"fightIDs":[1],"start":cursor as f64,"end":(start + 20) as f64
                }})))
                .respond_with(response(report(json!({"events":{
                    "data":[{"type":"cast","timestamp":timestamp,"abilityGameID":46387,"upload":code}],
                    "nextPageTimestamp":next
                }}))))
                .expect(1)
                .mount(&server)
                .await;
        }
    }
    let client = authenticate(&server);
    let directory = tempfile::tempdir().unwrap();
    let dir = directory.path();
    let mut collected = Vec::new();
    for (code, origin, start) in [("reportA", 1000, 10), ("reportB", 900, 110)] {
        let data = client.collect(code, 1).unwrap();
        assert_eq!(data["report"]["code"], code);
        assert_eq!(data["report"]["startTime"], origin);
        assert_eq!(
            data["events"],
            json!([
                {"type":"cast","timestamp":start + 1,"abilityGameID":46387,"upload":code},
                {"type":"cast","timestamp":start + 11,"abilityGameID":46387,"upload":code}
            ])
        );
        assert_eq!(data["collection"]["reportCode"], code);
        assert_eq!(
            data["collection"]["pageStartTimes"],
            json!([start as f64, (start + 10) as f64])
        );
        assert_eq!(
            data["collection"]["requests"]["events"]["variables"]["code"],
            code
        );
        save(&data, dir, &format!("{code}_1.json"), OutputFormat::Json).unwrap();
        collected.push(data);
    }
    assert_ne!(collected[0]["events"], collected[1]["events"]);
    for (code, expected) in ["reportA", "reportB"].into_iter().zip(&collected) {
        let saved: Value =
            serde_json::from_slice(&fs::read(dir.join(format!("{code}_1.json"))).unwrap()).unwrap();
        assert_eq!(&saved, expected);
    }
    server.verify().await;
}
