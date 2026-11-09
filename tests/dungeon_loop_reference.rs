use std::fs;

use assert_cmd::cargo::cargo_bin_cmd;
use serde_json::{Value, json};

#[rstest::rstest]
#[case(false)]
#[case(true)]
fn reference_loop_keeps_predicting_before_the_next_boundary_signal(#[case] missing_boundary: bool) {
    let directory = tempfile::tempdir().unwrap();
    let timeline = directory.path().join("reference.kdl");
    fs::write(
        &timeline,
        include_str!("fixtures/timeline/meso-terminal-loop.kdl"),
    )
    .unwrap();
    let mut recording: Value =
        serde_json::from_str(include_str!("fixtures/timeline/meso-terminal-loop.json")).unwrap();
    if missing_boundary {
        // A clock loop predicts the next round; this does not validate a missing required observation.
        recording["observations"]
            .as_array_mut()
            .unwrap()
            .retain(|row| row["at_ms"].as_i64().unwrap() < 50000);
        recording["end_ms"] = 180000.into();
    }
    let input = directory.path().join("recording.json");
    fs::write(&input, serde_json::to_vec(&recording).unwrap()).unwrap();
    let output = cargo_bin_cmd!("btimeline")
        .arg("compare")
        .arg(&timeline)
        .arg(&input)
        .arg("--json")
        .assert()
        .success();
    let result: Value = serde_json::from_slice(&output.get_output().stdout).unwrap();
    let rows = result["recordings"][input.to_str().unwrap()]
        .as_array()
        .unwrap();
    assert_eq!(
        rows.iter().filter(|row| row["action"] == "phase").count(),
        1
    );
    assert!(!rows.iter().any(|row| {
        ["ambiguous", "unresolved", "violation", "skipped"]
            .contains(&row["action"].as_str().unwrap())
    }));
    let events:Vec<_>=rows.iter().filter(|row|row["action"]=="event").map(|row|json!({"at_ms":row["at_ms"],"event":row["detail"].as_str().unwrap().split(':').next().unwrap()})).collect();
    assert_eq!(
        events
            .iter()
            .filter(|row| row["event"] == "rotation/main/loop/chirurgeon-rotation/no-mans-land")
            .count(),
        if missing_boundary { 4 } else { 2 }
    );
    insta::assert_json_snapshot!(
        if missing_boundary {
            "loop_forecast_without_boundary"
        } else {
            "loop_observed_cycle"
        },
        if missing_boundary {
            events
                .into_iter()
                .filter(|row| row["event"] == "rotation/main/loop/chirurgeon-rotation/no-mans-land")
                .collect::<Vec<_>>()
        } else {
            events
        }
    );
}
