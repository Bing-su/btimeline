use std::fs;

use assert_cmd::cargo::cargo_bin_cmd;
use serde_json::{Value, json};

#[rstest::rstest]
#[case(49700, 0)]
#[case(49700, 2000)]
#[case(250000, 0)]
#[case(250000, 2000)]
fn early_and_late_exit_cancel_cycles_and_keep_exit_clock_independent(
    #[case] exit: i64,
    #[case] exit_delay: i64,
) {
    let directory = tempfile::tempdir().expect("temporary directory");
    let timeline = directory.path().join("queen.kdl");
    fs::write(
        &timeline,
        include_str!("../timelines/queen-eternal-hold.kdl"),
    )
    .expect("standalone KDL");
    let recording = directory.path().join("recording.json");
    let mut observations =
        vec![json!({"at_ms":0,"type":"Ability","fields":{"id":"8F00","source":"Queen Eternal"}})];
    if exit > 60000 {
        // Shift the loop by 5s without changing the parent clock or the actual exit timestamp.
        observations.push(
            json!({"at_ms":59700,"type":"Ability","fields":{"id":"8EFA","source":"Queen Eternal"}}),
        );
    }
    observations.extend([
        json!({"at_ms":exit,"type":"StartsUsing","fields":{"id":"8EFF","source":"Queen Eternal"}}),
        json!({"at_ms":exit+5000+exit_delay,"type":"Ability","fields":{"id":"8EFF","source":"Queen Eternal"}}),
        json!({"at_ms":exit+68700+exit_delay,"type":"Ability","fields":{"id":"8F00","source":"Queen Eternal"}}),
    ]);
    fs::write(
        &recording,
        serde_json::to_vec(&json!({"version":1,"end_ms":exit+400000,"observations":observations}))
            .expect("encode recording"),
    )
    .expect("recording");
    let result = cargo_bin_cmd!("btimeline")
        .current_dir(directory.path())
        .arg("compare")
        .arg(timeline)
        .arg(&recording)
        .arg("--json")
        .assert()
        .success();
    let replay: Value = serde_json::from_slice(&result.get_output().stdout).expect("replay JSON");
    let rows = replay["recordings"][recording.to_str().expect("recording path")]
        .as_array()
        .expect("records");
    assert!(!rows.iter().any(|row| {
        ["ambiguous", "unresolved", "violation"].contains(&row["action"].as_str().expect("action"))
    }));
    assert_eq!(
        rows.iter().filter(|row| row["action"] == "phase").count(),
        1
    );
    let exits: Vec<_> = rows.iter().filter(|row| row["action"] == "until").collect();
    assert_eq!(exits.len(), 1);
    assert_eq!(exits.first().expect("exit")["at_ms"], exit);
    let events: Vec<_> = rows.iter().filter(|row| row["action"] == "event").collect();
    let hold: Vec<_> = events
        .iter()
        .filter(|row| row["detail"].as_str().expect("detail").starts_with("hold-"))
        .collect();
    assert!(
        hold.iter()
            .all(|row| row["at_ms"].as_i64().expect("time") < exit)
    );
    if exit < 54700 {
        assert!(
            hold.is_empty(),
            "A pre-start exit must skip even Prosecution"
        );
    } else {
        assert!(
            hold.iter()
                .filter(|row| row["detail"]
                    .as_str()
                    .expect("detail")
                    .starts_with("hold-prosecution:"))
                .count()
                >= 3
        );
        assert!(
            hold.iter().any(|row| row["at_ms"] == 74800),
            "The first Force must follow the corrected cycle clock"
        );
    }
    assert!(events.iter().any(|row| {
        row["detail"]
            .as_str()
            .expect("detail")
            .starts_with("exit-castellation-first:")
            && row["at_ms"] == exit + 13100 + exit_delay
    }));
    assert_eq!(
        events
            .iter()
            .filter(|row| row["detail"]
                .as_str()
                .expect("detail")
                .starts_with("exit-shift:"))
            .count(),
        1
    );
    assert!(
        events
            .iter()
            .all(|row| row["at_ms"].as_i64().expect("time") <= exit + 68700 + exit_delay),
        "The return to 8F00 must not restart the exited loop"
    );
}
