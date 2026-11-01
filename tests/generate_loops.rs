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

#[rstest::rstest]
#[case("dungeon", false)]
#[case("alliance", false)]
#[case("dungeon", true)]
fn generate_learns_a_loop_before_estimating_its_clock(#[case] mode: &str, #[case] no_opener: bool) {
    let dir = tempfile::tempdir().unwrap();
    let paths: Vec<_> = (0..3)
        .map(|peer| {
            let ids = if peer == 1 {
                vec![9, 1, 3, 2, 4, 1, 2, 3, 4, 1, 3, 2]
            } else {
                vec![9, 1, 2, 3, 4, 1, 3, 2, 4, 1, 2, 3]
            };
            fixture(
                dir.path(),
                &format!("loop{peer}"),
                &ids[usize::from(no_opener)..],
            )
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
    assert_eq!(repeated["start_ms"], if no_opener { 0 } else { 1000 });
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
    fs::write(&recording,serde_json::to_vec(&json!({"version":1,"end_ms":15000,"observations":[{"at_ms":0,"type":"Ability","fields":{"id":if no_opener {"1"} else {"9"},"source":"Synthetic Boss"}}]})).unwrap()).unwrap();
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
    assert_eq!(
        boundary,
        if no_opener {
            json!([0, 4000, 8000, 12000])
        } else {
            json!([1000, 5000, 9000, 13000])
        }
        .as_array()
        .unwrap()
        .clone()
    );
    if mode == "dungeon" && !no_opener {
        insta::assert_snapshot!(kdl);
    }
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
