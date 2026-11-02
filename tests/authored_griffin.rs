use std::fs;
use std::path::Path;

use assert_cmd::cargo::cargo_bin_cmd;
use serde_json::{Value, json};

#[rstest::rstest]
#[case(false)]
#[case(true)]
fn authored_griffin_predicts_without_evidence_and_resets(#[case] reset: bool) {
    let directory = tempfile::tempdir().expect("temporary directory");
    let timeline = directory.path().join("griffin.kdl");
    fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("timelines/mistwake-griffin.kdl"),
        &timeline,
    )
    .expect("copy standalone KDL");
    let recording = directory.path().join("recording.json");
    let mut observations = vec![
        json!({"at_ms":0,"type":"Ability","fields":{"id":"B0EB","source":"Thundergust Griffin"}}),
    ];
    if reset {
        observations.extend([
            json!({"at_ms":95000,"type":"InCombat","fields":{"inGameCombat":"0"}}),
            json!({"at_ms":120000,"type":"Ability","fields":{"id":"B0EB","source":"Thundergust Griffin"}}),
        ]);
    }
    fs::write(
        &recording,
        serde_json::to_vec(&json!({"version":1,"end_ms":if reset {140000} else {300000},"observations":observations})).expect("encode recording"),
    ).expect("write recording");
    let output = cargo_bin_cmd!("btimeline")
        .arg("compare")
        .arg(&timeline)
        .arg(&recording)
        .arg("--json")
        .assert()
        .success();
    let report: Value = serde_json::from_slice(&output.get_output().stdout).expect("replay JSON");
    let rows = report["recordings"][recording.to_str().expect("recording path")]
        .as_array()
        .expect("replay records");
    assert!(!rows.iter().any(|row| {
        ["ambiguous", "violation", "unresolved"].contains(&row["action"].as_str().expect("action"))
    }));
    let events: Vec<_> = rows.iter().filter(|row| row["action"] == "event").collect();
    if reset {
        assert!(!events.iter().any(|row| {
            row["at_ms"]
                .as_i64()
                .is_some_and(|at| (95000..120000).contains(&at))
        }));
        assert!(
            events
                .iter()
                .any(|row| row["at_ms"] == 131700 && row["run"] == 2)
        );
    } else {
        // A helper display is predicted from its parent mechanic; neither helper observations nor evidence are available.
        assert!(events.iter().any(|row| {
            row["at_ms"] == 273400
                && row["detail"]
                    .as_str()
                    .expect("event detail")
                    .contains("Electrifying Flight")
        }));
    }
}
