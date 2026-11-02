use std::fs;

use assert_cmd::cargo::cargo_bin_cmd;
use serde_json::Value;

const INPUT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/pull.json");

#[test]
fn inspect_defaults_to_a_human_summary_and_keeps_json_explicit() {
    let result = cargo_bin_cmd!("btimeline")
        .args(["inspect", INPUT])
        .assert()
        .success();
    insta::assert_snapshot!(
        "inspect_human_summary",
        String::from_utf8_lossy(&result.get_output().stdout)
    );
    let result = cargo_bin_cmd!("btimeline")
        .args(["inspect", INPUT, "--json"])
        .assert()
        .success();
    let groups: Value = serde_json::from_slice(&result.get_output().stdout).unwrap();
    let pull = &groups[0]["pulls"][0];
    assert_eq!(pull["event_count"], 3);
    assert_eq!(pull["end_seconds"], 20);
    assert_eq!(pull["occurrences"].as_array().unwrap().len(), 3);
    assert!(pull.get("end_ms").is_none());
}

#[rstest::rstest]
#[case(false)]
#[case(true)]
fn inspect_rejects_late_invalid_input_without_partial_output(#[case] json: bool) {
    let dir = tempfile::tempdir().unwrap();
    let invalid = dir.path().join("invalid.json");
    let mut data: Value = serde_json::from_slice(&fs::read(INPUT).unwrap()).unwrap();
    data["collection"]["complete"] = false.into();
    fs::write(&invalid, serde_json::to_vec(&data).unwrap()).unwrap();
    let mut command = cargo_bin_cmd!("btimeline");
    command.args(["inspect", INPUT]).arg(&invalid);
    if json {
        command.arg("--json");
    }
    let result = command.assert().failure();
    assert!(result.get_output().stdout.is_empty());
}

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
fn collection_rejects_unknown_purpose_before_authentication() {
    let directory = tempfile::tempdir().unwrap();
    let result = cargo_bin_cmd!("btimeline")
        .current_dir(directory.path())
        .args([
            "fflogs-collect",
            "example",
            "--purpose",
            "invalid",
            "-o",
            "plan.json",
            "--client-id",
            "test-id",
            "--client-secret",
            "private-test-value",
        ])
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&result.get_output().stderr);
    assert!(stderr.contains("Unknown collection purpose"));
    assert!(!stderr.contains("private-test-value"));
    assert!(!directory.path().join("plan.json").exists());
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
            .all(|n| n.children().unwrap().nodes().len() == 1)
    );
    let opening = activities
        .iter()
        .find(|n| {
            n.children().unwrap().nodes()[0]
                .get("id")
                .unwrap()
                .as_string()
                == Some("B529")
        })
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
        2
    );
    let traces: Vec<_> = nodes
        .iter()
        .filter(|n| n.name().value() == "trace")
        .collect();
    assert_eq!(traces.len(), 2);
    assert!(traces.iter().any(|n| {
        n.get("kill").unwrap().as_bool() == Some(false)
            && n.children()
                .unwrap()
                .nodes()
                .iter()
                .filter(|node| node.name().value() == "observed")
                .count()
                == 1
    }));
    cargo_bin_cmd!("btimeline")
        .arg("mine")
        .arg(&first_path)
        .arg("-o")
        .arg(&output)
        .assert()
        .failure();
    assert_eq!(fs::read_to_string(output).unwrap(), text);
}

#[test]
fn mine_combines_translated_names_from_different_source_languages() {
    let dir = tempfile::tempdir().unwrap();
    let other = dir.path().join("other.json");
    let output = dir.path().join("process.kdl");
    let mut data: Value = serde_json::from_slice(&fs::read(INPUT).unwrap()).unwrap();
    data["report"]["masterData"]["lang"] = "ja".into();
    data["report"]["code"] = "japanese-source".into();
    data["collection"]["reportCode"] = "japanese-source".into();
    fs::write(&other, serde_json::to_vec(&data).unwrap()).unwrap();
    cargo_bin_cmd!("btimeline")
        .arg("mine")
        .arg(INPUT)
        .arg(&other)
        .arg("-o")
        .arg(&output)
        .assert()
        .success();
    let document: kdl::KdlDocument = fs::read_to_string(output).unwrap().parse().unwrap();
    let root = &document.nodes()[0];
    assert!(root.get("language").is_none());
    let nodes = root.children().unwrap().nodes();
    let activities: Vec<_> = nodes
        .iter()
        .filter(|node| node.name().value() == "activity")
        .collect();
    assert_eq!(activities.len(), 1);
    assert_eq!(
        activities[0].get("observations").unwrap().as_integer(),
        Some(6)
    );
    let languages: std::collections::BTreeSet<_> = nodes
        .iter()
        .filter(|node| node.name().value() == "trace")
        .map(|node| node.get("source_language").unwrap().as_string().unwrap())
        .collect();
    assert_eq!(languages, ["en", "ja"].into_iter().collect());
}

#[rstest::rstest]
#[case(&[], true, 3)]
#[case(&[], false, 3)]
#[case(&[8000], true, 2)]
#[case(&[4000,8000], true, 0)]
fn mine_reports_observed_repeat_iterations_with_phase_boundaries(
    #[case] boundaries: &[i64],
    #[case] kill: bool,
    #[case] iterations: i128,
) {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("repeat.json");
    let output = dir.path().join("process.kdl");
    let mut data: Value = serde_json::from_slice(&fs::read(INPUT).unwrap()).unwrap();
    data["events"][1]["timestamp"] = 6001.into();
    data["report"]["fights"][0]["kill"] = kill.into();
    data["report"]["fights"][0]["phaseTransitions"] = boundaries
        .iter()
        .enumerate()
        .map(|(index, at)| serde_json::json!({"id":index + 1,"startTime":at}))
        .collect();
    fs::write(&input, serde_json::to_vec(&data).unwrap()).unwrap();
    cargo_bin_cmd!("btimeline")
        .arg("mine")
        .arg(&input)
        .arg("-o")
        .arg(&output)
        .assert()
        .success();
    let document: kdl::KdlDocument = fs::read_to_string(output).unwrap().parse().unwrap();
    let trace = document.nodes()[0]
        .children()
        .unwrap()
        .nodes()
        .iter()
        .find(|node| node.name().value() == "trace")
        .unwrap();
    let candidates: Vec<_> = trace
        .children()
        .unwrap()
        .nodes()
        .iter()
        .filter(|node| node.name().value() == "repeat-candidate")
        .collect();
    if iterations == 0 {
        assert!(candidates.is_empty());
        return;
    }
    assert_eq!(candidates.len(), 1);
    let repeat = candidates[0];
    assert_eq!(repeat.get("first_batch").unwrap().as_integer(), Some(0));
    assert_eq!(repeat.get("period_batches").unwrap().as_integer(), Some(1));
    assert_eq!(
        repeat.get("observed_iterations").unwrap().as_integer(),
        Some(iterations)
    );
    let samples = repeat.children().unwrap().nodes();
    assert_eq!(samples.len(), usize::try_from(iterations).unwrap());
    assert_eq!(samples[1].get("at_ms").unwrap().as_integer(), Some(5001));
    assert_eq!(samples[1].get("first_batch").unwrap().as_integer(), Some(1));
}

#[rstest::rstest]
#[case(false, false)]
#[case(true, false)]
#[case(false, true)]
fn mine_reports_common_progress_without_treating_wipes_as_missing_suffixes(
    #[case] reversed: bool,
    #[case] jitter: bool,
) {
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("process.kdl");
    let base: Value = serde_json::from_slice(&fs::read(INPUT).unwrap()).unwrap();
    let observations = [
        vec![
            (2000, 46376),
            (6000, 46377),
            (if jitter { 2001 } else { 10000 }, 46378),
            (15000, 46379),
        ],
        vec![
            (
                if jitter {
                    2001
                } else if reversed {
                    10001
                } else {
                    2100
                },
                46376,
            ),
            (
                if jitter {
                    2000
                } else if reversed {
                    2100
                } else {
                    10100
                },
                46378,
            ),
            (15100, 46379),
        ],
        vec![(2000, 46376)],
    ];
    let paths: Vec<_> = observations
        .iter()
        .enumerate()
        .map(|(index, events)| {
            let mut data = base.clone();
            let code = format!("pull{index}");
            data["report"]["code"] = code.clone().into();
            data["collection"]["reportCode"] = code.into();
            data["report"]["fights"][0]["kill"] = (index != 2).into();
            data["report"]["masterData"]["abilities"] = serde_json::json!([
            {"gameID":46376,"name":"A","type":"1"},
            {"gameID":46377,"name":"B","type":"1"},
            {"gameID":46378,"name":"C","type":"1"},
            {"gameID":46379,"name":"D","type":"1"}]);
            data["events"] = events
                .iter()
                .map(|(at, id)| {
                    serde_json::json!({
                        "timestamp":at,"type":"cast","sourceID":10,"abilityGameID":id,"fight":1
                    })
                })
                .collect();
            data["collection"]["eventCount"] = events.len().into();
            let path = dir.path().join(format!("{index}.json"));
            fs::write(&path, serde_json::to_vec(&data).unwrap()).unwrap();
            path
        })
        .collect();
    cargo_bin_cmd!("btimeline")
        .arg("mine")
        .args(&paths)
        .arg("-o")
        .arg(&output)
        .assert()
        .success();
    let document: kdl::KdlDocument = fs::read_to_string(output).unwrap().parse().unwrap();
    let common = document.nodes()[0]
        .children()
        .unwrap()
        .nodes()
        .iter()
        .find(|node| node.name().value() == "common-progress")
        .unwrap();
    assert_eq!(common.get("completed_pulls").unwrap().as_integer(), Some(2));
    assert_eq!(
        common.get("order_resolution_ms").unwrap().as_integer(),
        Some(100)
    );
    assert_eq!(
        common.get("status").unwrap().as_string(),
        Some(if reversed {
            "ambiguous-order"
        } else {
            "observed-common-order"
        })
    );
    let nodes = common.children().unwrap().nodes();
    let anchors: Vec<_> = nodes
        .iter()
        .filter(|node| node.name().value() == "anchor")
        .collect();
    assert_eq!(anchors.len(), 3);
    if jitter {
        assert_eq!(
            anchors
                .iter()
                .filter(|anchor| {
                    anchor.get("min_ms").and_then(kdl::KdlValue::as_integer) == Some(1000)
                        && anchor.get("max_ms").and_then(kdl::KdlValue::as_integer) == Some(1001)
                })
                .count(),
            2
        );
    }
    for anchor in anchors {
        assert_eq!(anchor.children().unwrap().nodes().len(), 2);
        assert!(
            anchor.get("min_ms").unwrap().as_integer()
                <= anchor.get("median_ms").unwrap().as_integer()
        );
        assert!(
            anchor.get("median_ms").unwrap().as_integer()
                <= anchor.get("max_ms").unwrap().as_integer()
        );
    }
    assert_eq!(
        nodes
            .iter()
            .filter(|node| node.name().value() == "step")
            .count(),
        0
    );
    assert_eq!(
        nodes
            .iter()
            .filter(|node| node.name().value() == "precedes")
            .count(),
        2
    );
}

#[rstest::rstest]
#[case(0)]
#[case(1)]
fn mine_preserves_common_signal_identity_with_an_extra_packet(#[case] offset_ms: i64) {
    let dir = tempfile::tempdir().unwrap();
    let other = dir.path().join("extra.json");
    let output = dir.path().join("process.kdl");
    let mut data: Value = serde_json::from_slice(&fs::read(INPUT).unwrap()).unwrap();
    data["report"]["code"] = "extra".into();
    data["collection"]["reportCode"] = "extra".into();
    data["report"]["masterData"]["abilities"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({"gameID":46377,"name":"B","type":"1"}));
    data["events"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "timestamp":2000 + offset_ms,"type":"cast","sourceID":10,"abilityGameID":46377,"fight":1
        }));
    data["collection"]["eventCount"] = 4.into();
    fs::write(&other, serde_json::to_vec(&data).unwrap()).unwrap();
    cargo_bin_cmd!("btimeline")
        .arg("mine")
        .arg(INPUT)
        .arg(&other)
        .arg("-o")
        .arg(&output)
        .assert()
        .success();
    let document: kdl::KdlDocument = fs::read_to_string(output).unwrap().parse().unwrap();
    let nodes = document.nodes()[0].children().unwrap().nodes();
    let activities: Vec<_> = nodes
        .iter()
        .filter(|node| node.name().value() == "activity")
        .collect();
    assert_eq!(activities.len(), 2);
    let common = activities
        .iter()
        .find(|node| {
            node.children().unwrap().nodes()[0]
                .get("id")
                .unwrap()
                .as_string()
                == Some("B528")
        })
        .unwrap();
    assert_eq!(common.get("observations").unwrap().as_integer(), Some(6));
    let extra = activities
        .iter()
        .find(|node| {
            node.children().unwrap().nodes()[0]
                .get("id")
                .unwrap()
                .as_string()
                == Some("B529")
        })
        .unwrap();
    assert_eq!(extra.get("observations").unwrap().as_integer(), Some(1));
    let within_batch_edge = nodes.iter().any(|node| {
        node.name().value() == "follows"
            && node.get(0) == common.get(0)
            && node.get(1) == extra.get(0)
    });
    assert_eq!(within_batch_edge, offset_ms != 0);
}

#[rstest::rstest]
#[case(1000)]
#[case(1555)]
#[case(21000)]
fn phase_boundaries_survive_inspection_and_mining(#[case] timestamp: i64) {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("phases.json");
    let output = dir.path().join("process.kdl");
    let mut data: Value = serde_json::from_slice(&fs::read(INPUT).unwrap()).unwrap();
    data["report"]["fights"][0]["phaseTransitions"] =
        serde_json::json!([{"id":2,"startTime":timestamp}]);
    fs::write(&input, serde_json::to_vec(&data).unwrap()).unwrap();
    let inspection = cargo_bin_cmd!("btimeline")
        .arg("inspect")
        .arg(&input)
        .arg("--json")
        .assert()
        .success();
    let inspected: Value = serde_json::from_slice(&inspection.get_output().stdout).unwrap();
    let phases = inspected[0]["pulls"][0]["phase_transitions"]
        .as_array()
        .unwrap();
    assert_eq!(phases.len(), 1);
    assert_eq!(phases[0]["id"], 2);
    for (field, expected_ms) in [
        ("timestamp_seconds", timestamp),
        ("relative_seconds", timestamp - 1000),
    ] {
        let actual = phases[0][field].as_f64().unwrap();
        assert!((actual - expected_ms as f64 / 1000.0).abs() < 1e-9);
    }
    cargo_bin_cmd!("btimeline")
        .arg("mine")
        .arg(&input)
        .arg("-o")
        .arg(&output)
        .assert()
        .success();
    let document: kdl::KdlDocument = fs::read_to_string(output).unwrap().parse().unwrap();
    let trace = document.nodes()[0]
        .children()
        .unwrap()
        .nodes()
        .iter()
        .find(|node| node.name().value() == "trace")
        .unwrap();
    let boundary = trace
        .children()
        .unwrap()
        .nodes()
        .iter()
        .find(|node| node.name().value() == "phase-boundary")
        .unwrap();
    assert_eq!(boundary.get(0).unwrap().as_integer(), Some(2));
    assert_eq!(
        boundary.get("at_ms").unwrap().as_integer(),
        Some(i128::from(timestamp - 1000))
    );
}

#[rstest::rstest]
#[case("applybuff")]
#[case("headmarker")]
#[case("tether")]
#[case("unknown-future-event")]
fn mine_retains_non_cast_packets_without_inventing_activities(#[case] kind: &str) {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("signals.json");
    let output = dir.path().join("process.kdl");
    let mut data: Value = serde_json::from_slice(&fs::read(INPUT).unwrap()).unwrap();
    let event = serde_json::json!({"timestamp":1555,"type":kind,"targetID":10,
        "extraData":{"values":[1,null,true,"text"]}});
    data["events"] = serde_json::json!([event]);
    data["collection"]["eventCount"] = 1.into();
    fs::write(&input, serde_json::to_vec(&data).unwrap()).unwrap();
    cargo_bin_cmd!("btimeline")
        .arg("mine")
        .arg(&input)
        .arg("-o")
        .arg(&output)
        .assert()
        .success();
    let document: kdl::KdlDocument = fs::read_to_string(output).unwrap().parse().unwrap();
    let nodes = document.nodes()[0].children().unwrap().nodes();
    assert!(!nodes.iter().any(|node| node.name().value() == "activity"));
    let trace = nodes
        .iter()
        .find(|node| node.name().value() == "trace")
        .unwrap();
    let packets = trace.children().unwrap().nodes();
    assert_eq!(packets.len(), 1);
    let packet = &packets[0];
    assert_eq!(packet.name().value(), "raw-event");
    assert_eq!(packet.get(0).unwrap().as_integer(), Some(0));
    assert_eq!(packet.get("at_ms").unwrap().as_integer(), Some(555));
    assert!(packet.get("source").is_none());
    assert_eq!(
        packet.get("target").unwrap().as_string(),
        Some("Synthetic Boss")
    );
    assert_eq!(
        serde_json::from_str::<Value>(packet.get("data").unwrap().as_string().unwrap()).unwrap(),
        event
    );
}

#[rstest::rstest]
#[case("headmarker", "markerID", "marker_id", true)]
#[case("headmarker", "markerID", "marker_id", false)]
#[case("tether", "tetherID", "tether_id", true)]
#[case("tether", "tetherID", "tether_id", false)]
#[case("applybuff", "abilityGameID", "id", true)]
#[case("refreshbuff", "abilityGameID", "id", true)]
#[case("removebuff", "abilityGameID", "id", true)]
#[case("applydebuff", "abilityGameID", "id", true)]
#[case("removedebuff", "abilityGameID", "id", true)]
#[case("refreshdebuff", "abilityGameID", "id", true)]
#[case("applybuff", "abilityGameID", "id", false)]
fn mine_selects_non_cast_candidates_without_player_names(
    #[case] kind: &str,
    #[case] raw_id: &str,
    #[case] field: &str,
    #[case] enemy_target: bool,
) {
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("process.kdl");
    let paths: Vec<_> = ["First Player", "Second Player"].iter().enumerate().map(|(index, name)| {
        let mut data: Value = serde_json::from_slice(&fs::read(INPUT).unwrap()).unwrap();
        let code = format!("candidate{index}");
        data["report"]["code"] = code.clone().into();
        data["collection"]["reportCode"] = code.into();
        data["report"]["masterData"]["actors"].as_array_mut().unwrap().push(
            serde_json::json!({"id":20,"gameID":1000020,"name":name,"type":"Player","subType":"BlackMage"}));
        let mut event = serde_json::json!({"timestamp":1555,"type":kind,"sourceID":20,
            "targetID":if enemy_target {10} else {20},"abilityGameID":0});
        event[raw_id] = 46376.into();
        data["events"] = serde_json::json!([event]);
        data["collection"]["eventCount"] = 1.into();
        let path = dir.path().join(format!("{index}.json"));
        fs::write(&path, serde_json::to_vec(&data).unwrap()).unwrap();
        path
    }).collect();
    cargo_bin_cmd!("btimeline")
        .arg("mine")
        .args(&paths)
        .arg("-o")
        .arg(&output)
        .assert()
        .success();
    let document: kdl::KdlDocument = fs::read_to_string(output).unwrap().parse().unwrap();
    let activities: Vec<_> = document.nodes()[0]
        .children()
        .unwrap()
        .nodes()
        .iter()
        .filter(|node| node.name().value() == "activity")
        .collect();
    if !enemy_target && field == "id" {
        assert!(activities.is_empty());
        return;
    }
    assert_eq!(activities.len(), 1);
    assert_eq!(
        activities[0].get("observations").unwrap().as_integer(),
        Some(2)
    );
    let signal = &activities[0].children().unwrap().nodes()[0];
    assert_eq!(signal.get(0).unwrap().as_string(), Some(kind));
    assert_eq!(signal.get(field).unwrap().as_string(), Some("B528"));
    assert!(signal.get("source").is_none());
    assert_eq!(
        signal.get("target").and_then(kdl::KdlValue::as_string),
        enemy_target.then_some("Synthetic Boss")
    );
}

#[test]
fn timeline_validation_and_replay_are_accessible_without_overwriting_inputs() {
    let timeline = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/timeline/basic.kdl"
    );
    let recording = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/timeline/basic.json"
    );
    let validation = cargo_bin_cmd!("btimeline")
        .args(["validate", timeline])
        .assert()
        .success();
    insta::assert_snapshot!(
        "timeline_validation_summary",
        String::from_utf8_lossy(&validation.get_output().stdout)
    );
    let replay = cargo_bin_cmd!("btimeline")
        .args(["compare", timeline, recording, "--json"])
        .assert()
        .success();
    let output: Value = serde_json::from_slice(&replay.get_output().stdout).unwrap();
    // Remove only the machine-specific input path; retain every timestamp and replay action.
    insta::assert_json_snapshot!(
        "timeline_basic_cli_replay",
        serde_json::json!({
            "records": output["recordings"][recording],
            "not_checked": output["not_checked"]
        })
    );
    cargo_bin_cmd!("btimeline")
        .args(["compare", timeline])
        .assert()
        .failure();
}

#[test]
fn compare_replays_branch_loop_and_phase_corrections_through_the_cli() {
    let timeline = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/timeline/branch-loop.kdl"
    );
    let recording = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/timeline/branch-loop.json"
    );
    let replay = cargo_bin_cmd!("btimeline")
        .args(["compare", timeline, recording, "--json"])
        .assert()
        .success();
    let output: Value = serde_json::from_slice(&replay.get_output().stdout).unwrap();
    insta::assert_json_snapshot!(
        "timeline_branch_loop_cli_replay",
        serde_json::json!({
            "records": output["recordings"][recording],
            "not_checked": output["not_checked"]
        })
    );
}

#[test]
fn compare_replays_option_loop_and_merge_through_the_cli() {
    let timeline = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/timeline/option-loop.kdl"
    );
    let recording = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/timeline/option-loop.json"
    );
    let replay = cargo_bin_cmd!("btimeline")
        .args(["compare", timeline, recording, "--json"])
        .assert()
        .success();
    let output: Value = serde_json::from_slice(&replay.get_output().stdout).unwrap();
    insta::assert_json_snapshot!(
        "timeline_option_loop_cli_replay",
        serde_json::json!({
            "records": output["recordings"][recording],
            "not_checked": output["not_checked"]
        })
    );
}

#[test]
fn compare_fails_without_partial_output_when_a_recording_is_invalid() {
    let timeline = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/timeline/branch-loop.kdl"
    );
    let recording = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/timeline/branch-loop.json"
    );
    let directory = tempfile::tempdir().unwrap();
    let invalid = directory.path().join("invalid.json");
    fs::write(
        &invalid,
        r#"{"version":1,"end_ms":1,"observations":[{"at_ms":2,"type":"X","fields":{}}]}"#,
    )
    .unwrap();
    let result = cargo_bin_cmd!("btimeline")
        .args(["compare", timeline, recording])
        .arg(&invalid)
        .arg("--json")
        .assert()
        .failure();
    assert!(result.get_output().stdout.is_empty());
    assert!(
        String::from_utf8_lossy(&result.get_output().stderr)
            .contains("Observations must be in original time order")
    );
}

#[test]
fn compare_accepts_native_fflogs_events_and_retains_source_indices() {
    let timeline = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/timeline/fflogs-native.kdl"
    );
    let result = cargo_bin_cmd!("btimeline")
        .args(["compare", timeline, INPUT, "--json"])
        .assert()
        .success();
    let output: Value = serde_json::from_slice(&result.get_output().stdout).unwrap();
    let records = output["recordings"][INPUT].as_array().unwrap();
    assert!(
        records
            .iter()
            .any(|r| r["action"] == "start" && r["at_ms"] == 1000 && r["source_event_index"] == 0)
    );
    assert!(
        records
            .iter()
            .any(|r| r["action"] == "phase" && r["source_event_index"] == 0)
    );
    assert!(
        records
            .iter()
            .any(|r| r["action"] == "sync" && r["at_ms"] == 5000 && r["source_event_index"] == 1)
    );
    assert!(records.iter().any(|r| r["action"] == "event"
        && r["at_ms"] == 5000
        && r.get("source_event_index").is_none()));
    assert!(
        output["not_checked"]
            .as_array()
            .unwrap()
            .contains(&Value::from("FFLogs-to-ACT signal equivalence"))
    );
}

#[test]
fn compare_rejects_incomplete_fflogs_input() {
    let timeline = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/timeline/fflogs-native.kdl"
    );
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("invalid.json");
    let mut incomplete: Value = serde_json::from_slice(&fs::read(INPUT).unwrap()).unwrap();
    incomplete["collection"]["complete"] = false.into();
    fs::write(&path, serde_json::to_vec(&incomplete).unwrap()).unwrap();
    let result = cargo_bin_cmd!("btimeline")
        .args(["compare", timeline])
        .arg(&path)
        .arg("--json")
        .assert()
        .failure();
    assert!(result.get_output().stdout.is_empty());
    assert!(String::from_utf8_lossy(&result.get_output().stderr).contains("Incomplete collection"));
}

#[rstest::rstest]
#[case("basic", "basic.json", "replayed", 2)]
#[case("basic", "empty.json", "no_observations", 0)]
#[case("basic", "fflogs", "not_started", 0)]
#[case("fflogs-native", "fflogs", "replayed", 1)]
fn compare_reports_whether_replay_started(
    #[case] timeline: &str,
    #[case] recording: &str,
    #[case] status: &str,
    #[case] started_runs: usize,
) {
    let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/timeline");
    let directory = tempfile::tempdir().unwrap();
    let path = match recording {
        "fflogs" => std::path::PathBuf::from(INPUT),
        "empty.json" => {
            let path = directory.path().join(recording);
            fs::write(&path, r#"{"version":1,"end_ms":0,"observations":[]}"#).unwrap();
            path
        }
        _ => fixtures.join(recording),
    };
    let result = cargo_bin_cmd!("btimeline")
        .arg("compare")
        .arg(fixtures.join(format!("{timeline}.kdl")))
        .arg(&path)
        .arg("--json")
        .assert()
        .success();
    let output: Value = serde_json::from_slice(&result.get_output().stdout).unwrap();
    let summary = &output["summaries"][path.to_str().unwrap()];
    assert_eq!(summary["status"], status);
    assert_eq!(summary["started_runs"], started_runs);
    assert_eq!(summary["observations"] == 0, status == "no_observations");
}

#[test]
fn compare_orders_late_fflogs_events_without_losing_original_indices() {
    let timeline = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/timeline/fflogs-native.kdl"
    );
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("unordered.json");
    let mut data: Value = serde_json::from_slice(&fs::read(INPUT).unwrap()).unwrap();
    data["events"].as_array_mut().unwrap().swap(0, 1);
    fs::write(&path, serde_json::to_vec(&data).unwrap()).unwrap();
    let result = cargo_bin_cmd!("btimeline")
        .args(["compare", timeline])
        .arg(&path)
        .arg("--json")
        .assert()
        .success();
    let output: Value = serde_json::from_slice(&result.get_output().stdout).unwrap();
    let records = output["recordings"][path.to_str().unwrap()]
        .as_array()
        .unwrap();
    assert!(
        records
            .iter()
            .any(|r| r["action"] == "start" && r["at_ms"] == 1000 && r["source_event_index"] == 1)
    );
    assert!(
        records
            .iter()
            .any(|r| r["action"] == "sync" && r["at_ms"] == 5000 && r["source_event_index"] == 0)
    );
}
