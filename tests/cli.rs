use std::fs;

use assert_cmd::cargo::cargo_bin_cmd;
use serde_json::Value;

const INPUT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/pull.json");

#[test]
fn inspect_rejects_invalid_input_and_preserves_json_boundaries() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("input.json");
    let original: Value = serde_json::from_slice(&fs::read(INPUT).unwrap()).unwrap();
    let mut invalid = Vec::new();
    for (pointer, value) in [
        ("/collection/complete", serde_json::json!(false)),
        ("/collection/eventCount", serde_json::json!(999)),
        ("/collection/schemaVersion", serde_json::json!(2)),
        ("/collection/startTime", serde_json::json!(-1)),
        ("/collection/endTime", serde_json::json!(999)),
        ("/collection/pageStartTimes", serde_json::json!([])),
        (
            "/collection/pageStartTimes",
            serde_json::json!([1000, 1000]),
        ),
        ("/collection/reportCode", serde_json::json!("wrong")),
        ("/collection/pageCount", serde_json::json!(2)),
        ("/collection/nextPageTimestamp", serde_json::json!(1500)),
        ("/events/0/timestamp", serde_json::json!(21001)),
        ("/events/0/abilityGameID", serde_json::json!(42)),
        ("/events/0/fight", serde_json::json!(3)),
        ("/events/0/timestamp", serde_json::json!("invalid")),
    ] {
        let mut data = original.clone();
        *data.pointer_mut(pointer).unwrap() = value;
        invalid.push(data);
    }
    for transitions in [
        serde_json::json!({}),
        serde_json::json!([{"id":"one","startTime":1000}]),
        serde_json::json!([{"id":1,"startTime":999}]),
        serde_json::json!([{"id":1,"startTime":21001}]),
        serde_json::json!([{"id":1,"startTime":1000},{"id":2,"startTime":1000}]),
        serde_json::json!([{"id":1,"startTime":1500},{"id":2,"startTime":1400}]),
    ] {
        let mut data = original.clone();
        data["report"]["fights"][0]["phaseTransitions"] = transitions;
        invalid.push(data);
    }
    for data in invalid {
        fs::write(&input, serde_json::to_vec(&data).unwrap()).unwrap();
        cargo_bin_cmd!("btimeline")
            .args(["inspect", input.to_str().unwrap()])
            .assert()
            .failure();
    }
    let mut wrong_type = original.clone();
    wrong_type["events"][0]["timestamp"] = "invalid".into();
    fs::write(&input, serde_json::to_vec(&wrong_type).unwrap()).unwrap();
    let error = cargo_bin_cmd!("btimeline")
        .args(["inspect", input.to_str().unwrap()])
        .assert()
        .failure();
    assert!(String::from_utf8_lossy(&error.get_output().stderr).contains("events[0].timestamp"));
    let bytes = serde_json::to_vec(&original).unwrap();
    for suffix in ["{}", "garbage"] {
        fs::write(&input, [bytes.as_slice(), suffix.as_bytes()].concat()).unwrap();
        cargo_bin_cmd!("btimeline")
            .args(["inspect", input.to_str().unwrap()])
            .assert()
            .failure();
    }
    fs::write(&input, [bytes.as_slice(), b" \n\t"].concat()).unwrap();
    cargo_bin_cmd!("btimeline")
        .args(["inspect", input.to_str().unwrap()])
        .assert()
        .success();
}

#[test]
fn fflogs_tools_load_dotenv_without_exposing_credentials() {
    let directory = tempfile::tempdir().expect("temporary credentials directory");
    fs::write(
        directory.path().join(".env"),
        "FFLOGS_CLIENT_ID=test-id\nFFLOGS_CLIENT_SECRET='private-test-value'\n",
    )
    .expect("local credentials fixture");
    for arguments in [
        vec!["fflogs", "invalid/code", "1"],
        vec!["fflogs-collect", "-o", "plan.json"],
    ] {
        let result = cargo_bin_cmd!("btimeline")
            .current_dir(directory.path())
            .env_remove("FFLOGS_CLIENT_ID")
            .env_remove("FFLOGS_CLIENT_SECRET")
            .args(arguments)
            .assert()
            .failure();
        let stderr = String::from_utf8_lossy(&result.get_output().stderr);
        assert!(!stderr.contains("private-test-value"));
        assert!(!stderr.contains("Missing required flag"));
    }
    // A malformed secret is never included in a dotenv parser error.
    fs::write(
        directory.path().join(".env"),
        "FFLOGS_CLIENT_SECRET='private-test-value\n",
    )
    .expect("malformed fixture");
    let result = cargo_bin_cmd!("btimeline")
        .current_dir(directory.path())
        .args(["fflogs-collect", "--help"])
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&result.get_output().stderr);
    assert!(stderr.contains("Cannot load .env"));
    assert!(!stderr.contains("private-test-value"));
}

#[test]
fn mine_preserves_wipes_simultaneous_events_and_existing_output() {
    let dir = tempfile::tempdir().unwrap();
    let mut first: Value = serde_json::from_slice(&fs::read(INPUT).unwrap()).unwrap();
    first["report"]["masterData"]["abilities"] = serde_json::json!([
        {"gameID": 46376, "name": "A", "type": "1"},
        {"gameID": 46377, "name": "B", "type": "1"}
    ]);
    first["events"] = serde_json::json!([
        {"timestamp":2000,"type":"cast","sourceID":10,"abilityGameID":46376,"fight":1},
        {"timestamp":2000,"type":"cast","sourceID":10,"abilityGameID":46377,"fight":1},
        {"timestamp":6000,"type":"cast","sourceID":10,"abilityGameID":46376,"fight":1}
    ]);
    first["collection"]["eventCount"] = 3.into();
    let mut wipe = first.clone();
    wipe["report"]["code"] = "wipe".into();
    wipe["collection"]["reportCode"] = "wipe".into();
    wipe["report"]["fights"][0]["kill"] = false.into();
    wipe["events"].as_array_mut().unwrap().pop();
    wipe["collection"]["eventCount"] = 2.into();
    let first_path = dir.path().join("first.json");
    let wipe_path = dir.path().join("wipe.json");
    let output = dir.path().join("process.kdl");
    fs::write(&first_path, serde_json::to_vec(&first).unwrap()).unwrap();
    fs::write(&wipe_path, serde_json::to_vec(&wipe).unwrap()).unwrap();
    cargo_bin_cmd!("btimeline")
        .arg("mine")
        .arg(&first_path)
        .arg(&wipe_path)
        .arg("-o")
        .arg(&output)
        .assert()
        .success();
    let text = fs::read_to_string(&output).unwrap();
    let document: kdl::KdlDocument = text.parse().unwrap();
    let root = document.nodes().first().unwrap();
    let nodes = root.children().unwrap().nodes();
    let activities: Vec<_> = nodes
        .iter()
        .filter(|n| n.name().value() == "activity")
        .collect();
    assert_eq!(activities.len(), 2);
    assert!(
        activities
            .iter()
            .any(|n| n.children().unwrap().nodes().len() == 2)
    );
    let opening = activities
        .iter()
        .find(|n| n.children().unwrap().nodes().len() == 2)
        .unwrap();
    assert_eq!(opening.get("first_seen").unwrap().as_integer(), Some(2));
    assert_eq!(
        opening.get("last_seen_in_kills").unwrap().as_integer(),
        Some(0)
    );
    assert_eq!(
        opening.children().unwrap().nodes()[0]
            .get("source")
            .unwrap()
            .as_string(),
        Some("Synthetic Boss")
    );
    assert_eq!(
        nodes
            .iter()
            .filter(|n| n.name().value() == "follows")
            .count(),
        1
    );
    let traces: Vec<_> = nodes
        .iter()
        .filter(|n| n.name().value() == "trace")
        .collect();
    assert_eq!(traces.len(), 2);
    assert!(
        traces
            .iter()
            .any(|n| n.get("kill").unwrap().as_bool() == Some(false)
                && n.children().unwrap().nodes().len() == 1)
    );
    cargo_bin_cmd!("btimeline")
        .arg("mine")
        .arg(&first_path)
        .arg("-o")
        .arg(&output)
        .assert()
        .failure();
    assert_eq!(fs::read_to_string(output).unwrap(), text);
}
