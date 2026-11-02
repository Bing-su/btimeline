use std::fs;

use assert_cmd::cargo::cargo_bin_cmd;
use serde_json::{Value, json};

const TIMELINE: &str = r#"
timeline 1 "Forecast test" {
    start "Ability" id="START"
    finish "Ability" id="FINISH"
    phase "first" {
        enter "Ability" id="START"
        event "hidden" 3.0 name="Hidden" visible=#false
        event "common" 5.0 name="Common" duration=2.0
        branch "choice" open=0.0 close=10.0 {
            option "left" {
                when "Ability" id="LEFT"
                event "selected" 2.0 name="Selected"
            }
            option "right" {
                when "Ability" id="RIGHT"
                event "unselected" 2.0 name="Unselected"
            }
            merge {
                when "Ability" id="MERGE"
                event "merged" 1.0 name="Merged"
            }
        }
        loop "rotation" start=30.0 every=10.0 {
            until "Ability" id="EXIT" at=32.0 before=32.0 after=100.0
            event "pulse" 0.0 name="Pulse"
            on-exit { event "after-exit" 1.0 name="After exit"; }
        }
    }
    phase "later" {
        enter "Ability" id="LATER" from="first"
        event "later-hit" 2.0 name="Later"
    }
}
"#;

fn query(observations: Value, end: i64, at: i64, horizon: i64) -> Value {
    let dir = tempfile::tempdir().expect("temporary directory");
    let timeline = dir.path().join("timeline.kdl");
    let recording = dir.path().join("recording.json");
    fs::write(&timeline, TIMELINE).expect("timeline");
    fs::write(
        &recording,
        serde_json::to_vec(&json!({"version":1,"end_ms":end,"observations":observations}))
            .expect("encode recording"),
    )
    .expect("recording");
    let output = cargo_bin_cmd!("btimeline")
        .arg("forecast")
        .arg(timeline)
        .arg(recording)
        .arg("--at-ms")
        .arg(at.to_string())
        .arg("--horizon-ms")
        .arg(horizon.to_string())
        .assert()
        .success();
    serde_json::from_slice(&output.get_output().stdout).expect("forecast JSON")
}

#[test]
fn forecast_excludes_future_signals_and_unselected_paths() {
    let first = query(
        json!([
            {"at_ms":0,"type":"Ability","fields":{"id":"START"}},
            {"at_ms":2000,"type":"Ability","fields":{"id":"LEFT"}},
            {"at_ms":4500,"type":"Ability","fields":{"id":"FINISH"}}
        ]),
        20000,
        2000,
        10000,
    );
    let other = query(
        json!([
            {"at_ms":0,"type":"Ability","fields":{"id":"START"}},
            {"at_ms":2000,"type":"Ability","fields":{"id":"LEFT"}},
            {"at_ms":4500,"type":"Ability","fields":{"id":"LATER"}}
        ]),
        20000,
        2000,
        10000,
    );
    assert_eq!(first, other);
    assert_eq!(first["status"], "active");
    assert_eq!(first["phase"], "first");
    assert_eq!(first["observations_used"], 2);
    assert_eq!(
        first["events"],
        json!([
            {"id":"selected","name":"Selected","at_ms":4000,"lead_ms":2000,"duration_ms":0,"run":1},
            {"id":"common","name":"Common","at_ms":5000,"lead_ms":3000,"duration_ms":2000,"run":1}
        ])
    );
}

#[test]
fn future_branch_timeout_is_not_a_present_diagnostic() {
    let forecast = query(
        json!([{"at_ms":0,"type":"Ability","fields":{"id":"START"}}]),
        20000,
        0,
        12000,
    );
    assert_eq!(forecast["diagnostics"], json!([]));
    assert_eq!(
        forecast["projection_diagnostics"][0]["action"],
        "unresolved"
    );
    // 9.950s is the first raw time that rounds into the authored 10.0s close boundary.
    assert_eq!(forecast["projection_diagnostics"][0]["at_ms"], 9950);
    assert_eq!(forecast["events"].as_array().expect("events").len(), 1);
    assert_eq!(forecast["events"][0]["id"], "common");
}

#[test]
fn forecast_extends_repeats_past_the_recording_end() {
    let forecast = query(
        json!([{"at_ms":0,"type":"Ability","fields":{"id":"START"}}]),
        35000,
        35000,
        25000,
    );
    assert_eq!(
        forecast["events"]
            .as_array()
            .expect("events")
            .iter()
            .map(|row| row["at_ms"].clone())
            .collect::<Vec<_>>(),
        vec![json!(40000), json!(50000), json!(60000)]
    );
    assert_eq!(forecast["diagnostics"][0]["action"], "unresolved");
}

#[test]
fn observed_merge_and_exit_keep_their_authored_event_identity() {
    let merged = query(
        json!([
            {"at_ms":0,"type":"Ability","fields":{"id":"START"}},
            {"at_ms":2000,"type":"Ability","fields":{"id":"LEFT"}},
            {"at_ms":3000,"type":"Ability","fields":{"id":"MERGE"}}
        ]),
        35000,
        3000,
        2000,
    );
    assert_eq!(merged["events"][0]["id"], "merged");
    let exited = query(
        json!([
            {"at_ms":0,"type":"Ability","fields":{"id":"START"}},
            {"at_ms":32000,"type":"Ability","fields":{"id":"EXIT"}}
        ]),
        35000,
        32000,
        15000,
    );
    assert_eq!(exited["events"].as_array().expect("events").len(), 1);
    assert_eq!(exited["events"][0]["id"], "after-exit");
    assert_eq!(exited["events"][0]["at_ms"], 33000);
}

#[test]
fn all_same_time_observations_are_applied_before_the_query() {
    let forecast = query(
        json!([
            {"at_ms":0,"type":"Ability","fields":{"id":"START"}},
            {"at_ms":2000,"type":"Ability","fields":{"id":"LEFT"}},
            {"at_ms":2000,"type":"Ability","fields":{"id":"FINISH"}}
        ]),
        20000,
        2000,
        10000,
    );
    assert_eq!(forecast["status"], "idle");
    assert_eq!(forecast["events"], json!([]));
    assert_eq!(forecast["observations_used"], 3);
}

#[test]
fn invalid_cutoffs_and_horizons_are_rejected() {
    let dir = tempfile::tempdir().expect("temporary directory");
    let timeline = dir.path().join("timeline.kdl");
    let recording = dir.path().join("recording.json");
    fs::write(&timeline, TIMELINE).expect("timeline");
    fs::write(
        &recording,
        r#"{"version":1,"end_ms":20000,"observations":[]}"#,
    )
    .expect("recording");
    for (at, horizon, reason) in [
        (20001, 1000_i64, "inside the recording"),
        (0, 0, "positive"),
        (1000, 10_000_000_000_000, "supported time range"),
        (0, 20_000_000_000, "one million"),
    ] {
        let output = cargo_bin_cmd!("btimeline")
            .arg("forecast")
            .arg(&timeline)
            .arg(&recording)
            .arg("--at-ms")
            .arg(at.to_string())
            .arg("--horizon-ms")
            .arg(horizon.to_string())
            .assert()
            .failure();
        assert!(String::from_utf8_lossy(&output.get_output().stderr).contains(reason));
    }
}
