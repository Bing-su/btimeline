use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use assert_cmd::cargo::cargo_bin_cmd;
use serde_json::{Value, json};

fn fixture(directory: &Path, code: &str, ids: &[i64]) -> PathBuf {
    let mut data: Value =
        serde_json::from_str(include_str!("fixtures/pull.json")).expect("fixture");
    let events:Vec<_>=ids.iter().enumerate().map(|(index,id)|json!({"timestamp":2000+index*1000,"type":"cast","sourceID":10,"abilityGameID":id,"fight":1})).collect();
    let abilities = json!([
        {"gameID":9,"name":"Opening","type":"1"},
        {"gameID":8,"name":"Loop Entry","type":"1"},
        {"gameID":7,"name":"Tutorial","type":"1"},
        {"gameID":1,"name":"Boundary","type":"1"},
        {"gameID":2,"name":"Left","type":"1"},
        {"gameID":3,"name":"Right","type":"1"},
        {"gameID":4,"name":"Closing","type":"1"}
    ]);
    for (pointer, value) in [
        ("/report/code", code.into()),
        ("/collection/reportCode", code.into()),
        ("/report/masterData/abilities", abilities),
        ("/collection/eventCount", events.len().into()),
        ("/events", events.into()),
    ] {
        *data.pointer_mut(pointer).expect("fixture field") = value;
    }
    let path = directory.join(format!("{code}.json"));
    fs::write(&path, serde_json::to_vec(&data).expect("encode fixture")).expect("write fixture");
    path
}

// A combat loop can span named helpers even when FFLogs identifies no Boss actor.
fn with_helpers(path: PathBuf) -> PathBuf {
    let mut data: Value =
        serde_json::from_slice(&fs::read(&path).expect("read fixture")).expect("parse fixture");
    *data
        .pointer_mut("/report/masterData/actors/0/subType")
        .expect("actor subtype") = "NPC".into();
    for (id, name) in [(11, "Left Helper"), (12, "Right Helper")] {
        data.pointer_mut("/report/masterData/actors")
            .expect("actors")
            .as_array_mut()
            .expect("actor list")
            .push(json!({"id":id,"name":name,"gameID":99000+id,"type":"NPC","subType":"NPC"}));
        data.pointer_mut("/report/fights/0/enemyNPCs")
            .expect("enemies")
            .as_array_mut()
            .expect("enemy list")
            .push(json!({"id":id,"gameID":99000+id}));
    }
    for event in data
        .get_mut("events")
        .and_then(Value::as_array_mut)
        .expect("events")
    {
        match event
            .get("abilityGameID")
            .and_then(Value::as_i64)
            .expect("ability")
        {
            2 => *event.get_mut("sourceID").expect("source") = 11.into(),
            3 => *event.get_mut("sourceID").expect("source") = 12.into(),
            _ => {}
        }
    }
    fs::write(&path, serde_json::to_vec(&data).expect("encode fixture")).expect("write fixture");
    path
}

#[rstest::rstest]
#[case("complementary", true)]
#[case("same-entity", false)]
#[case("no-witness", false)]
fn loop_fragments_need_per_position_witnesses_and_disjoint_entities(
    #[case] scenario: &str,
    #[case] expected: bool,
) {
    let dir = tempfile::tempdir().unwrap();
    let paths: Vec<_> = (0..2)
        .map(|peer| {
            let path = with_helpers(fixture(
                dir.path(),
                &format!("split{peer}"),
                &[9, 1, 2, 3, 4, 1, 2, 3],
            ));
            let mut data: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            let events = data["events"].as_array_mut().unwrap();
            for event in events.iter_mut() {
                event["sourceInstance"] = 1.into();
            }
            let fragmented = if scenario == "no-witness" {
                1
            } else {
                peer + 1
            };
            let extra: Vec<_> = events
                .iter()
                .filter(|event| event["abilityGameID"] == fragmented)
                .map(|event| {
                    let mut event = event.clone();
                    event["timestamp"] = (event["timestamp"].as_i64().unwrap() + 50).into();
                    event["sourceInstance"] = if scenario == "same-entity" { 1 } else { 2 }.into();
                    event
                })
                .collect();
            events.extend(extra);
            data["collection"]["eventCount"] = data["events"].as_array().unwrap().len().into();
            fs::write(&path, serde_json::to_vec(&data).unwrap()).unwrap();
            path
        })
        .collect();
    let output = dir.path().join("timeline.kdl");
    cargo_bin_cmd!("btimeline")
        .arg("generate")
        .args(paths)
        .args(["--mode", "raid", "-o"])
        .arg(&output)
        .assert()
        .success();
    let e: Value =
        serde_json::from_slice(&fs::read(output.with_extension("evidence.json")).unwrap()).unwrap();
    assert_eq!(
        e["loops"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["status"] == "rendered"),
        expected
    );
    if expected {
        let rows = e["loops"][0]["events"].as_array().unwrap();
        assert_eq!(rows.len(), 4);
        let members: usize = rows
            .iter()
            .flat_map(|row| row["observations"].as_array().unwrap())
            .map(|observation| observation["members"].as_array().unwrap().len())
            .sum();
        assert_eq!(members, 18);
    }
}

#[rstest::rstest]
#[case("dungeon", false)]
#[case("alliance", false)]
#[case("dungeon", true)]
#[case("raid", false)]
fn generate_learns_a_loop_before_estimating_its_clock(#[case] mode: &str, #[case] no_opener: bool) {
    let dir = tempfile::tempdir().unwrap();
    let paths: Vec<_> = (0..3)
        .map(|peer| {
            let ids = if peer == 1 {
                vec![9, 1, 3, 2, 4, 1, 2, 3, 4, 1, 3, 2]
            } else {
                vec![9, 1, 2, 3, 4, 1, 3, 2, 4, 1, 2, 3]
            };
            let path = fixture(
                dir.path(),
                &format!("loop{peer}"),
                &ids[usize::from(no_opener)..],
            );
            if mode == "raid" {
                with_helpers(path)
            } else {
                path
            }
        })
        .collect();
    let output = dir.path().join("timeline.kdl");
    cargo_bin_cmd!("btimeline")
        .arg("generate")
        .args(paths)
        .args(["--mode", mode, "-o"])
        .arg(&output)
        .assert()
        .success();
    let evidence: Value =
        serde_json::from_slice(&fs::read(output.with_extension("evidence.json")).unwrap()).unwrap();
    let loops = evidence["loops"].as_array().unwrap();
    assert_eq!(loops.len(), 1);
    let repeated = &loops[0];
    assert_eq!(repeated["status"], "rendered");
    assert_eq!(repeated["period_ms"], 4000);
    let combat_offset = if mode == "raid" { 1000 } else { 0 };
    assert_eq!(
        repeated["start_ms"],
        combat_offset + if no_opener { 0 } else { 1000 }
    );
    assert_eq!(repeated["events"].as_array().unwrap().len(), 4);
    for trace in evidence["progression"]["traces"].as_array().unwrap() {
        let covered: BTreeSet<_> = repeated["events"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|event| event["observations"].as_array().unwrap())
            .filter(|observation| observation["trace"] == trace["trace"])
            .flat_map(|observation| observation["observation_nodes"].as_array().unwrap())
            .map(|node| node.as_u64().unwrap())
            .collect();
        assert_eq!(covered.len(), 11);
    }
    let kdl = fs::read_to_string(&output).unwrap();
    assert!(!kdl.contains("count="));
    assert!(kdl.contains("Left / Right"));
    fs::remove_file(output.with_extension("evidence.json")).unwrap();
    let recording = dir.path().join("recording.json");
    // With no later observations, future rounds must still be predicted from the loop clock.
    let observations = if mode == "raid" {
        json!([{"at_ms":0,"type":"InCombat","fields":{"inGameCombat":"1"}}])
    } else {
        json!([{"at_ms":0,"type":"Ability","fields":{"id":if no_opener {"1"} else {"9"},"source":"Synthetic Boss"}}])
    };
    fs::write(
        &recording,
        serde_json::to_vec(&json!({"version":1,"end_ms":15000,"observations":observations}))
            .unwrap(),
    )
    .unwrap();
    let result = cargo_bin_cmd!("btimeline")
        .arg("compare")
        .arg(&output)
        .arg(&recording)
        .arg("--json")
        .assert()
        .success();
    let result: Value = serde_json::from_slice(&result.get_output().stdout).unwrap();
    let rows = result["recordings"][recording.to_str().unwrap()]
        .as_array()
        .unwrap();
    assert!(!rows.iter().any(|row| {
        ["violation", "ambiguous", "unresolved"].contains(&row["action"].as_str().unwrap())
    }));
    let boundary: Vec<_> = rows
        .iter()
        .filter(|row| {
            row["action"] == "event" && row["detail"].as_str().unwrap().contains("01-boundary")
        })
        .map(|row| row["at_ms"].clone())
        .collect();
    let start = combat_offset + if no_opener { 0 } else { 1000 };
    assert_eq!(
        boundary,
        (0..4)
            .map(|index| Value::from(start + index * 4000))
            .collect::<Vec<_>>()
    );
    if mode == "dungeon" && !no_opener {
        insta::assert_snapshot!(kdl);
    }
}

#[rstest::rstest]
#[case("raid", false)]
#[case("dungeon", false)]
#[case("alliance", false)]
#[case("dungeon", true)]
fn representative_repeats_accept_different_wipe_lengths_and_predict_unseen_rounds(
    #[case] mode: &str,
    #[case] no_opener: bool,
) {
    let dir = tempfile::tempdir().unwrap();
    let paths: Vec<_> = [
        ("alpha", vec![9, 1, 2, 3, 4, 1, 2, 3]),
        ("beta", vec![9, 1, 3, 2, 4, 1, 2, 3, 4, 1, 3, 2]),
    ]
    .into_iter()
    .map(|(code, ids)| {
        let path = fixture(dir.path(), code, &ids[usize::from(no_opener)..]);
        let mut data: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        data["report"]["fights"][0]["kill"] = false.into();
        fs::write(&path, serde_json::to_vec(&data).unwrap()).unwrap();
        path
    })
    .collect();
    let output = dir.path().join("timeline.kdl");
    cargo_bin_cmd!("btimeline")
        .arg("generate")
        .args(&paths)
        .args(["--mode", mode, "-o"])
        .arg(&output)
        .assert()
        .success();
    let evidence: Value =
        serde_json::from_slice(&fs::read(output.with_extension("evidence.json")).unwrap()).unwrap();
    assert_eq!(evidence["generation_method"], "representative-route-v1");
    let route = &evidence["draft"]["routes"][0];
    assert_eq!(route["peers_complete"], true);
    assert_eq!(
        route["nodes"].as_array().unwrap().len(),
        if no_opener { 1 } else { 2 }
    );
    assert_eq!(route["repeat"]["entry"], if no_opener { 0 } else { 1 });
    assert_eq!(route["repeat"]["body"].as_array().unwrap().len(), 4);
    for (trace, count) in [("alpha/1", 7), ("beta/1", 11)] {
        let waves = route["repeat"]["observed_waves"][trace].as_array().unwrap();
        assert_eq!(waves.len(), count);
        let references: BTreeSet<_> = route["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|node| node["observation_nodes"][trace].as_u64())
            .chain(
                waves
                    .iter()
                    .flat_map(|wave| wave.as_array().unwrap())
                    .map(|node| node.as_u64().unwrap()),
            )
            .collect();
        assert_eq!(references.len(), count + usize::from(!no_opener));
    }
    let repeated = &evidence["loops"][0];
    assert_eq!(repeated["status"], "rendered");
    assert_eq!(
        repeated["replaced_finite_events"].as_array().unwrap().len(),
        1
    );
    let reversed = dir.path().join("reverse.kdl");
    cargo_bin_cmd!("btimeline")
        .arg("generate")
        .args(paths.into_iter().rev())
        .args(["--mode", mode, "-o"])
        .arg(&reversed)
        .assert()
        .success();
    assert_eq!(fs::read(&output).unwrap(), fs::read(&reversed).unwrap());
    fs::remove_file(output.with_extension("evidence.json")).unwrap();
    let recording = dir.path().join("recording.json");
    let observations = if mode == "raid" {
        json!([{"at_ms":0,"type":"InCombat","fields":{"inGameCombat":"1"}}])
    } else {
        json!([{"at_ms":0,"type":"Ability","fields":{"id":if no_opener {"1"} else {"9"},"source":"Synthetic Boss"}}])
    };
    fs::write(
        &recording,
        serde_json::to_vec(&json!({"version":1,"end_ms":23000,"observations":observations}))
            .unwrap(),
    )
    .unwrap();
    let result = cargo_bin_cmd!("btimeline")
        .arg("compare")
        .arg(&output)
        .arg(&recording)
        .arg("--json")
        .assert()
        .success();
    let result: Value = serde_json::from_slice(&result.get_output().stdout).unwrap();
    let rows = result["recordings"][recording.to_str().unwrap()]
        .as_array()
        .unwrap();
    assert!(!rows.iter().any(|row| {
        ["violation", "ambiguous", "unresolved"].contains(&row["action"].as_str().unwrap())
    }));
    assert!(
        rows.iter()
            .any(|row| row["action"] == "event" && row["at_ms"].as_i64().unwrap() > 20000)
    );
}

#[test]
fn a_representative_repeat_timing_failure_does_not_publish_a_truncated_route() {
    let dir = tempfile::tempdir().unwrap();
    let paths: Vec<_> = ["alpha", "beta"]
        .into_iter()
        .map(|code| {
            let path = fixture(dir.path(), code, &[9, 1, 2, 3, 4, 1, 2, 3, 4, 1, 2, 3, 4]);
            let mut data: Value =
                serde_json::from_slice(&fs::read(&path).expect("fixture")).expect("fixture JSON");
            // Two observed periods have lower median 4s, but the final complete cycle's last step needs 11s.
            for (event, at) in data
                .pointer_mut("/events")
                .expect("events")
                .as_array_mut()
                .expect("events array")
                .iter_mut()
                .zip([
                    2000, 3000, 4000, 5000, 6000, 7000, 11000, 15000, 18000, 19000, 23000, 27000,
                    30000,
                ])
            {
                *event.get_mut("timestamp").expect("timestamp") = at.into();
            }
            for (pointer, value) in [
                ("/report/fights/0/kill", false.into()),
                ("/report/fights/0/endTime", 35000.into()),
                ("/collection/endTime", 35000.into()),
                ("/report/endTime", 35000.into()),
            ] {
                *data.pointer_mut(pointer).expect("fixture field") = value;
            }
            fs::write(&path, serde_json::to_vec(&data).expect("encode fixture"))
                .expect("write fixture");
            path
        })
        .collect();
    let output = dir.path().join("timeline.kdl");
    let result = cargo_bin_cmd!("btimeline")
        .arg("generate")
        .args(paths)
        .args(["--mode", "raid", "-o"])
        .arg(&output)
        .assert()
        .failure();
    assert!(
        String::from_utf8_lossy(&result.get_output().stderr)
            .contains("terminal repeat could not be rendered")
    );
    assert!(!output.exists());
    assert!(!output.with_extension("evidence.json").exists());
}

#[test]
fn a_representative_choice_can_join_a_shared_terminal_repeat() {
    let dir = tempfile::tempdir().unwrap();
    let paths: Vec<_> = [
        ("alpha", vec![9, 2, 3, 8, 1, 4, 7, 1, 4, 7]),
        ("beta", vec![9, 3, 2, 8, 1, 4, 7, 1, 4, 7, 1, 4]),
    ]
    .into_iter()
    .map(|(code, ids)| {
        let path = fixture(dir.path(), code, &ids);
        let mut data: Value =
            serde_json::from_slice(&fs::read(&path).expect("fixture")).expect("fixture JSON");
        *data.pointer_mut("/report/fights/0/kill").expect("kill") = false.into();
        fs::write(&path, serde_json::to_vec(&data).expect("encode fixture"))
            .expect("write fixture");
        path
    })
    .collect();
    for mode in ["raid", "dungeon"] {
        let output = dir.path().join(format!("{mode}.kdl"));
        cargo_bin_cmd!("btimeline")
            .arg("generate")
            .args(&paths)
            .args(["--mode", mode, "-o"])
            .arg(&output)
            .assert()
            .success();
        let evidence: Value =
            serde_json::from_slice(&fs::read(output.with_extension("evidence.json")).unwrap())
                .unwrap();
        let route = &evidence["draft"]["routes"][0];
        assert_eq!(route["choices"].as_array().unwrap().len(), 1);
        assert!(route["repeat"]["entry"].is_number());
        assert_eq!(evidence["loops"][0]["status"], "rendered");
        let kdl = fs::read_to_string(&output).unwrap();
        assert_eq!(kdl.matches("branch ").count(), 1);
        assert_eq!(kdl.matches("loop ").count(), 1);
        fs::remove_file(output.with_extension("evidence.json")).unwrap();
        let recording = dir.path().join(format!("{mode}-recording.json"));
        let start = if mode == "raid" {
            json!({"at_ms":0,"type":"InCombat","fields":{"inGameCombat":"1"}})
        } else {
            json!({"at_ms":0,"type":"Ability","fields":{"id":"9","source":"Synthetic Boss"}})
        };
        let select = if mode == "raid" { 2000 } else { 1000 };
        fs::write(&recording,serde_json::to_vec(&json!({"version":1,"end_ms":23000,"observations":[start,{"at_ms":select,"type":"Ability","fields":{"id":"3","source":"Synthetic Boss"}}]})).unwrap()).unwrap();
        let result = cargo_bin_cmd!("btimeline")
            .arg("compare")
            .arg(&output)
            .arg(&recording)
            .arg("--json")
            .assert()
            .success();
        let result: Value = serde_json::from_slice(&result.get_output().stdout).unwrap();
        let rows = result["recordings"][recording.to_str().unwrap()]
            .as_array()
            .unwrap();
        assert!(
            !rows
                .iter()
                .any(|row| ["violation", "ambiguous", "unresolved"]
                    .contains(&row["action"].as_str().unwrap())),
            "{rows:?}"
        );
        assert!(rows.iter().any(|row| row["action"] == "event"
            && row["at_ms"] == select
            && row["detail"].as_str().unwrap().contains("Right")));
        assert!(
            rows.iter()
                .any(|row| row["action"] == "event" && row["at_ms"].as_i64().unwrap() > 20000)
        );
    }
}

#[rstest::rstest]
#[case("raid", "partial", "censored")]
#[case("dungeon", "partial", "censored")]
#[case("alliance", "complete", "censored")]
#[case("raid", "longer", "censored")]
#[case("raid", "before-entry", "censored")]
#[case("raid", "overdue", "unresolved")]
#[case("dungeon", "overdue", "unresolved")]
#[case("raid", "missing-step", "unresolved")]
#[case("raid", "missing-boundary", "unresolved")]
#[case("raid", "unseen-order", "unresolved")]
#[case("raid", "uncertain", "unresolved")]
#[case("raid", "choice", "censored")]
#[case("dungeon", "choice", "censored")]
#[case("raid", "compound", "unresolved")]
fn repeat_holdouts_relearn_without_the_entire_report(
    #[case] mode: &str,
    #[case] scenario: &str,
    #[case] status: &str,
) {
    let dir = tempfile::tempdir().unwrap();
    let ids = match scenario {
        "before-entry" => vec![9],
        "missing-step" => vec![9, 1, 2, 4, 1, 2, 3],
        "missing-boundary" => vec![9, 1, 2, 3, 4, 2, 3, 4],
        "unseen-order" => vec![9, 1, 3, 2, 4, 1, 3, 2],
        "complete" => vec![9, 1, 2, 3, 4, 1, 2, 3, 4],
        "longer" => vec![9, 1, 2, 3, 4, 1, 2, 3, 4, 1, 2, 3, 4, 1, 2, 3],
        "choice" => vec![9, 3, 2, 8, 1, 4],
        _ => vec![9, 1, 2, 3, 4, 1, 2],
    };
    let (alpha, beta) = if scenario == "choice" {
        (
            vec![9, 2, 3, 8, 1, 4, 7, 1, 4, 7],
            vec![9, 3, 2, 8, 1, 4, 7, 1, 4, 7],
        )
    } else {
        (
            vec![9, 1, 2, 3, 4, 1, 2, 3],
            vec![9, 1, 2, 3, 4, 1, 2, 3, 4, 1, 2, 3],
        )
    };
    let mut paths = vec![
        fixture(dir.path(), "alpha", &alpha),
        fixture(dir.path(), "beta", &beta),
        fixture(dir.path(), "zeta", &ids),
    ];
    if scenario == "compound" {
        for path in paths.iter().take(2) {
            let mut data: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
            let events = data["events"].as_array_mut().unwrap();
            let extra: Vec<_> = events
                .iter()
                .filter(|event| event["abilityGameID"] == 2)
                .cloned()
                .map(|mut event| {
                    event["abilityGameID"] = 7.into();
                    event
                })
                .collect();
            events.extend(extra);
            events.sort_by_key(|event| event["timestamp"].as_i64().unwrap());
            data["collection"]["eventCount"] = data["events"].as_array().unwrap().len().into();
            fs::write(path, serde_json::to_vec(&data).unwrap()).unwrap();
        }
    }
    let path = paths.last().unwrap();
    let mut data: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    if scenario == "uncertain" {
        data["events"][2]["timestamp"] = 3050.into();
    }
    let end = 2000 + (ids.len() - 1) * 1000 + if scenario == "overdue" { 1500 } else { 500 };
    if mode == "raid" {
        for pointer in [
            "/report/endTime",
            "/report/fights/0/endTime",
            "/collection/endTime",
        ] {
            *data.pointer_mut(pointer).unwrap() = end.into();
        }
    } else {
        data["events"]
            .as_array_mut()
            .unwrap()
            .push(json!({"timestamp":end,"type":"death","targetID":10,"fight":1}));
        data["collection"]["eventCount"] = data["events"].as_array().unwrap().len().into();
    }
    fs::write(path, serde_json::to_vec(&data).unwrap()).unwrap();
    // A second fight in the same report must be withheld too, not become another training supporter.
    data["report"]["fights"][0]["id"] = 2.into();
    data["collection"]["fightID"] = 2.into();
    for event in data["events"].as_array_mut().unwrap() {
        event["fight"] = 2.into();
    }
    let second = dir.path().join("zeta-second.json");
    fs::write(&second, serde_json::to_vec(&data).unwrap()).unwrap();
    paths.push(second);
    let output = dir.path().join("timeline.kdl");
    cargo_bin_cmd!("btimeline")
        .arg("generate")
        .args(paths)
        .args(["--mode", mode, "-o"])
        .arg(&output)
        .assert()
        .success();
    let evidence: Value =
        serde_json::from_slice(&fs::read(output.with_extension("evidence.json")).unwrap()).unwrap();
    let validation = evidence["alignment_diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["method"] == "automatic-draft-validation-v1")
        .unwrap();
    assert_eq!(validation["status"], "evaluated");
    assert_eq!(validation["split"]["held_out"], json!(["zeta/1", "zeta/2"]));
    assert!(!validation["model"].to_string().contains("zeta/"));
    assert!(validation["model"]["routes"][0]["repeat"].is_object());
    let checks = validation["checks"].as_array().unwrap();
    assert_eq!(checks.len(), 2);
    for check in checks {
        assert_eq!(check["method"], "observed-repeat-cycles-v1");
        assert_eq!(check["status"], status, "{check}");
        assert!(check["supporting_traces"].as_u64().unwrap() <= 2);
        if ["missing-step", "missing-boundary", "unseen-order"].contains(&scenario) {
            assert_eq!(
                check["complete_cycles"],
                if scenario == "missing-boundary" { 1 } else { 0 }
            );
        }
        if status == "censored" {
            assert!(check["scope_end_ms"].as_i64() < check["next_expected_ms"].as_i64());
        }
    }
}

#[test]
fn a_holdout_cannot_supply_the_second_training_witness_for_a_repeat() {
    let dir = tempfile::tempdir().unwrap();
    let paths = [
        fixture(dir.path(), "alpha", &[9, 1, 2, 3, 4, 1, 2, 3]),
        fixture(dir.path(), "beta", &[9, 1, 2]),
        fixture(dir.path(), "zeta", &[9, 1, 2, 3, 4, 1, 2, 3, 4, 1, 2, 3]),
    ];
    let output = dir.path().join("timeline.kdl");
    cargo_bin_cmd!("btimeline")
        .arg("generate")
        .args(paths)
        .args(["--mode", "raid", "-o"])
        .arg(&output)
        .assert()
        .success();
    let evidence: Value =
        serde_json::from_slice(&fs::read(output.with_extension("evidence.json")).unwrap()).unwrap();
    assert!(evidence["draft"]["routes"][0]["repeat"].is_object());
    let validation = evidence["alignment_diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["method"] == "automatic-draft-validation-v1")
        .unwrap();
    assert!(validation["model"]["routes"][0]["repeat"].is_null());
    assert!(!validation["model"].to_string().contains("zeta/1"));
    assert_eq!(validation["checks"][0]["status"], "unresolved");
}

#[test]
fn coincident_same_name_abilities_keep_distinct_event_ids() {
    let dir = tempfile::tempdir().unwrap();
    let paths: Vec<_> = (0..3)
        .map(|peer| {
            let path = fixture(
                dir.path(),
                &format!("coincident{peer}"),
                &[9, 1, 2, 3, 4, 1, 2, 3, 4, 1, 2, 3],
            );
            let mut data: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            data["report"]["masterData"]["abilities"]
                .as_array_mut()
                .unwrap()
                .push(json!({"gameID":5,"name":"Boundary","type":"1"}));
            let events = data["events"].as_array_mut().unwrap();
            let peers: Vec<_> = events
                .iter()
                .filter(|event| event["abilityGameID"] == 1)
                .map(|event| {
                    let mut peer = event.clone();
                    peer["abilityGameID"] = 5.into();
                    peer
                })
                .collect();
            events.extend(peers);
            data["collection"]["eventCount"] = data["events"].as_array().unwrap().len().into();
            fs::write(&path, serde_json::to_vec(&data).unwrap()).unwrap();
            path
        })
        .collect();
    let output = dir.path().join("timeline.kdl");
    cargo_bin_cmd!("btimeline")
        .arg("generate")
        .args(paths)
        .args(["--mode", "dungeon", "-o"])
        .arg(&output)
        .assert()
        .success();
    let evidence: Value =
        serde_json::from_slice(&fs::read(output.with_extension("evidence.json")).unwrap()).unwrap();
    let loops = evidence["loops"].as_array().unwrap();
    assert_eq!(loops.len(), 1);
    assert_eq!(loops[0]["status"], "rendered");
    let boundaries: Vec<_> = loops[0]["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|event| event["event"].as_str().unwrap().contains("boundary"))
        .collect();
    assert_eq!(boundaries.len(), 2);
    assert_ne!(boundaries[0]["event"], boundaries[1]["event"]);
}

#[rstest::rstest]
#[case("raid", 3000)]
#[case("dungeon", 2000)]
fn shared_local_context_allows_different_tutorial_lengths(#[case] mode: &str, #[case] start: i64) {
    let dir = tempfile::tempdir().unwrap();
    let paths: Vec<_> = [
        vec![9, 8, 1, 2, 3, 4, 1, 2, 3],
        vec![9, 7, 8, 1, 2, 3, 4, 1, 2, 3],
    ]
    .iter()
    .enumerate()
    .map(|(peer, ids)| fixture(dir.path(), &format!("tutorial{peer}"), ids))
    .collect();
    let output = dir.path().join("timeline.kdl");
    cargo_bin_cmd!("btimeline")
        .arg("generate")
        .args(paths)
        .args(["--mode", mode, "-o"])
        .arg(&output)
        .assert()
        .success();
    let evidence: Value =
        serde_json::from_slice(&fs::read(output.with_extension("evidence.json")).unwrap()).unwrap();
    let model = &evidence["loops"][0];
    assert_eq!(model["status"], "rendered");
    assert_eq!(model["model"]["context"]["identifier"]["Ability"], 8);
    assert_eq!(model["model"]["prefix"].as_array().unwrap().len(), 1);
    // The existing estimator chooses the lower median of the equally weighted trace offsets.
    assert_eq!(model["start_ms"], start);
    assert_eq!(model["period_ms"], 4000);
}

#[test]
fn raid_loop_does_not_hide_a_missing_internal_step() {
    let dir = tempfile::tempdir().unwrap();
    let paths: Vec<_> = [
        vec![9, 1, 2, 3, 4, 1, 2, 3],
        vec![9, 1, 2, 3, 4, 1, 2, 3],
        vec![9, 1, 2, 4, 1, 2, 3],
    ]
    .iter()
    .enumerate()
    .map(|(peer, ids)| with_helpers(fixture(dir.path(), &format!("missing{peer}"), ids)))
    .collect();
    let output = dir.path().join("timeline.kdl");
    cargo_bin_cmd!("btimeline")
        .arg("generate")
        .args(paths)
        .args(["--mode", "raid", "-o"])
        .arg(&output)
        .assert()
        .success();
    let evidence: Value =
        serde_json::from_slice(&fs::read(output.with_extension("evidence.json")).unwrap()).unwrap();
    assert!(evidence["loops"].as_array().unwrap().is_empty());
    assert!(
        evidence["loop_diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["status"] == "unresolved")
    );
    assert!(!evidence["events"].as_array().unwrap().is_empty());
}

#[test]
fn coincident_casters_of_the_same_ability_keep_distinct_loop_events() {
    let dir = tempfile::tempdir().unwrap();
    let paths: Vec<_> = (0..3)
        .map(|peer| {
            let path = with_helpers(fixture(
                dir.path(),
                &format!("casters{peer}"),
                &[9, 1, 2, 3, 4, 1, 2, 3, 4, 1, 2, 3],
            ));
            let mut data: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            let events = data["events"].as_array_mut().unwrap();
            let peers: Vec<_> = events
                .iter()
                .filter(|event| event["abilityGameID"] == 1)
                .map(|event| {
                    let mut event = event.clone();
                    event["sourceID"] = 11.into();
                    event
                })
                .collect();
            events.extend(peers);
            data["collection"]["eventCount"] = data["events"].as_array().unwrap().len().into();
            fs::write(&path, serde_json::to_vec(&data).unwrap()).unwrap();
            path
        })
        .collect();
    let output = dir.path().join("timeline.kdl");
    cargo_bin_cmd!("btimeline")
        .arg("generate")
        .args(paths)
        .args(["--mode", "raid", "-o"])
        .arg(&output)
        .assert()
        .success();
    let evidence: Value =
        serde_json::from_slice(&fs::read(output.with_extension("evidence.json")).unwrap()).unwrap();
    let repeated = &evidence["loops"][0];
    assert_eq!(repeated["status"], "rendered");
    let boundaries: Vec<_> = repeated["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|event| event["activities"][0]["identifier"]["Ability"] == 1)
        .collect();
    assert_eq!(boundaries.len(), 2);
    assert_ne!(boundaries[0]["event"], boundaries[1]["event"]);
    assert_ne!(
        boundaries[0]["activities"][0]["source"],
        boundaries[1]["activities"][0]["source"]
    );
}

#[test]
fn repeated_internal_abilities_keep_their_positions_and_raw_occurrences() {
    let dir = tempfile::tempdir().unwrap();
    let paths: Vec<_> = (0..3)
        .map(|peer| {
            fixture(
                dir.path(),
                &format!("repeated{peer}"),
                &[9, 1, 2, 3, 2, 4, 1, 2, 3, 2, 4, 1, 2, 3, 2],
            )
        })
        .collect();
    let output = dir.path().join("timeline.kdl");
    cargo_bin_cmd!("btimeline")
        .arg("generate")
        .args(paths)
        .args(["--mode", "dungeon", "-o"])
        .arg(&output)
        .assert()
        .success();
    let evidence: Value =
        serde_json::from_slice(&fs::read(output.with_extension("evidence.json")).unwrap()).unwrap();
    let repeated = &evidence["loops"][0];
    assert_eq!(repeated["status"], "rendered");
    assert_eq!(repeated["period_ms"], 5000);
    let events = repeated["events"].as_array().unwrap();
    assert_eq!(events.len(), 5);
    assert_eq!(events[1]["activities"], events[3]["activities"]);
    assert_ne!(events[1]["event"], events[3]["event"]);
    assert_eq!(events[1]["offset_ms"], 1000);
    assert_eq!(events[3]["offset_ms"], 3000);
    // Overlapping same-ID windows cannot independently identify these two nearby positions.
    assert_eq!(events[1]["sync"], false);
    assert_eq!(events[3]["sync"], false);
    for trace in evidence["progression"]["traces"].as_array().unwrap() {
        let covered: BTreeSet<_> = events
            .iter()
            .flat_map(|event| event["observations"].as_array().unwrap())
            .filter(|observation| observation["trace"] == trace["trace"])
            .flat_map(|observation| observation["observation_nodes"].as_array().unwrap())
            .map(|node| node.as_u64().unwrap())
            .collect();
        assert_eq!(covered.len(), 14);
    }
}
