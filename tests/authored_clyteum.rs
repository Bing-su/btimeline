use std::fs;

use assert_cmd::cargo::cargo_bin_cmd;
use serde_json::{Value, json};

#[rstest::rstest]
#[case("BF00", "Eye of the Scorpion", "eye-eyes-first", 306000)]
#[case("BEF4", "Chort", "chort-emission", 305200)]
#[case("BF18", "Malphas", "malphas-pair", 337700)]
#[case("BF18", "Malphas", "malphas-pair-helpers", 339300)]
fn each_boss_predicts_future_cycles_and_restarts_after_finish(
    #[case] entry: &str,
    #[case] source: &str,
    #[case] future_id: &str,
    #[case] future_at: i64,
) {
    let directory = tempfile::tempdir().expect("temporary directory");
    let timeline = directory.path().join("clyteum.kdl");
    let recording = directory.path().join("recording.json");
    fs::write(&timeline, include_str!("../timelines/clyteum.kdl")).expect("standalone KDL");
    for reset in [false, true] {
        let mut observations =
            vec![json!({"at_ms":0,"type":"Ability","fields":{"id":entry,"source":source}})];
        if reset {
            observations.extend([
                json!({"at_ms":30000,"type":"InCombat","fields":{"inGameCombat":"0"}}),
                json!({"at_ms":100000,"type":"Ability","fields":{"id":entry,"source":source}}),
            ]);
        }
        fs::write(&recording,serde_json::to_vec(&json!({"version":1,"end_ms":if reset{120000}else{400000},"observations":observations})).expect("encode recording")).expect("recording");
        let result = cargo_bin_cmd!("btimeline")
            .current_dir(directory.path())
            .arg("compare")
            .arg(&timeline)
            .arg(&recording)
            .arg("--json")
            .assert()
            .success();
        let replay: Value =
            serde_json::from_slice(&result.get_output().stdout).expect("replay JSON");
        let rows = replay["recordings"][recording.to_str().expect("recording path")]
            .as_array()
            .expect("records");
        assert!(!rows.iter().any(|row| {
            ["ambiguous", "unresolved", "violation"]
                .contains(&row["action"].as_str().expect("action"))
        }));
        let events: Vec<_> = rows.iter().filter(|row| row["action"] == "event").collect();
        if reset {
            assert!(
                !events.iter().any(
                    |row| (30000..100000).contains(&row["at_ms"].as_i64().expect("event time"))
                )
            );
            assert!(
                events
                    .iter()
                    .any(|row| row["at_ms"] == 100000 && row["run"] == 2)
            );
        } else {
            // Only the entry signal is supplied; later loop displays must not depend on evidence or later cast observations.
            assert!(events.iter().any(|row| {
                row["at_ms"] == future_at
                    && row["detail"]
                        .as_str()
                        .expect("detail")
                        .starts_with(&format!("{future_id}:"))
            }));
        }
    }
}

#[rstest::rstest]
#[case(
    "BF00",
    "Eye of the Scorpion",
    "BF00",
    132400,
    143000,
    "eye-beam",
    148000
)]
#[case("BEF4", "Chort", "C4F1", 167200, 167800, "chort-pressure", 172800)]
#[case("BF18", "Malphas", "BF1B", 84700, 87500, "malphas-void-first", 92500)]
#[case("BF18", "Malphas", "BF1B", 80700, 81200, "malphas-pair-helpers", 82300)]
fn delayed_cycle_markers_restore_forecasts_without_future_mechanic_observations(
    #[case] entry: &str,
    #[case] source: &str,
    #[case] anchor: &str,
    #[case] observed: i64,
    #[case] cutoff: i64,
    #[case] expected_id: &str,
    #[case] expected_at: i64,
) {
    let directory = tempfile::tempdir().expect("temporary directory");
    let timeline = directory.path().join("clyteum.kdl");
    let recording = directory.path().join("recording.json");
    fs::write(&timeline, include_str!("../timelines/clyteum.kdl")).expect("standalone KDL");
    for marker in [false, true] {
        let mut observations =
            vec![json!({"at_ms":0,"type":"Ability","fields":{"id":entry,"source":source}})];
        if marker {
            observations.push(
                json!({"at_ms":observed,"type":"Ability","fields":{"id":anchor,"source":source}}),
            );
        }
        fs::write(
            &recording,
            serde_json::to_vec(&json!({"version":1,"end_ms":200000,"observations":observations}))
                .expect("encode recording"),
        )
        .expect("recording");
        let result = cargo_bin_cmd!("btimeline")
            .current_dir(directory.path())
            .arg("forecast")
            .arg(&timeline)
            .arg(&recording)
            .arg("--at-ms")
            .arg(cutoff.to_string())
            .args(["--horizon-ms", "6500"])
            .assert()
            .success();
        let forecast: Value =
            serde_json::from_slice(&result.get_output().stdout).expect("forecast JSON");
        assert_eq!(forecast["diagnostics"], json!([]));
        assert_eq!(forecast["projection_diagnostics"], json!([]));
        // The corrected schedule is knowable only after the delayed marker; an absent observation must not count as recovery.
        assert_eq!(
            forecast["events"]
                .as_array()
                .expect("events")
                .iter()
                .any(|row| row["id"] == expected_id && row["at_ms"] == expected_at),
            marker
        );
    }
}
