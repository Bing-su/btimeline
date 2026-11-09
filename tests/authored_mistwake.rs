use std::fs;

use assert_cmd::cargo::cargo_bin_cmd;
use serde_json::{Value, json};

#[rstest::rstest]
#[case("SystemLogMessage", "id", "7DE")]
#[case("InCombat", "inGameCombat", "0")]
#[case("ActorControl", "command", "4000000F")]
#[case("ChangeZone", "id", "1314")]
fn all_bosses_restart_after_control_signals_and_cancel_travel_predictions(
    #[case] kind: &str,
    #[case] field: &str,
    #[case] value: &str,
) {
    let directory = tempfile::tempdir().expect("temporary directory");
    let timeline = directory.path().join("mistwake.kdl");
    fs::write(&timeline, include_str!("../timelines/mistwake.kdl")).expect("standalone timeline");
    let recording = directory.path().join("recording.json");
    let observations = json!([
        {"at_ms":0,"type":"Ability","fields":{"id":"A93F","source":"Treno Catoblepas"}},
        {"at_ms":100000,"type":kind,"fields":{field:value}},
        {"at_ms":300000,"type":"Ability","fields":{"id":"B11D","source":"Amdusias"}},
        {"at_ms":450000,"type":kind,"fields":{field:value}},
        {"at_ms":600000,"type":"Ability","fields":{"id":"B0EB","source":"Thundergust Griffin"}},
        {"at_ms":780000,"type":kind,"fields":{field:value}}
    ]);
    fs::write(
        &recording,
        serde_json::to_vec(&json!({"version":1,"end_ms":900000,"observations":observations}))
            .expect("encode recording"),
    )
    .expect("write recording");
    let result = cargo_bin_cmd!("btimeline")
        .current_dir(directory.path())
        .arg("compare")
        .arg(&timeline)
        .arg(&recording)
        .arg("--json")
        .assert()
        .success();
    let report: Value = serde_json::from_slice(&result.get_output().stdout).expect("replay JSON");
    let rows = report["recordings"][recording.to_str().expect("recording path")]
        .as_array()
        .expect("records");
    assert!(!rows.iter().any(|row| {
        ["unresolved", "ambiguous", "violation"].contains(&row["action"].as_str().expect("action"))
    }));
    let phases: Vec<_> = rows
        .iter()
        .filter(|row| row["action"] == "phase")
        .map(|row| json!([row["at_ms"], row["detail"], row["run"]]))
        .collect();
    assert_eq!(
        phases,
        vec![
            json!([0, "treno/main", 1]),
            json!([300000, "amdusias/main", 2]),
            json!([600000, "griffin/main", 3])
        ]
    );
    // Arena unseal and other explicit controls must cancel old loops throughout travel and after the final boss.
    for row in rows.iter().filter(|row| row["action"] == "event") {
        let at = row["at_ms"].as_i64().expect("event time");
        assert!(!(100000..300000).contains(&at) && !(450000..600000).contains(&at) && at < 780000);
    }
    assert!(
        rows.iter()
            .any(|row| row["action"] == "event" && row["at_ms"] == 312400 && row["run"] == 2)
    );
    assert!(
        rows.iter()
            .any(|row| row["action"] == "event" && row["at_ms"] == 611700 && row["run"] == 3)
    );
}
