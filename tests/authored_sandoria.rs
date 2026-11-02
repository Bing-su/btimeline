use std::fs;

use assert_cmd::cargo::cargo_bin_cmd;
use serde_json::{Value, json};

fn run(verb: &str, observations: Vec<Value>, at_ms: u64, horizon_ms: u64) -> Value {
    let directory = tempfile::tempdir().expect("temporary directory");
    let timeline = directory.path().join("sandoria.kdl");
    let recording = directory.path().join("recording.json");
    fs::write(&timeline, include_str!("../timelines/sandoria.kdl")).expect("standalone timeline");
    fs::write(
        &recording,
        serde_json::to_vec(
            &json!({"version":1,"end_ms":at_ms+horizon_ms,"observations":observations}),
        )
        .expect("JSON"),
    )
    .expect("recording");
    let mut command = cargo_bin_cmd!("btimeline");
    command
        .current_dir(directory.path())
        .arg(verb)
        .arg(&timeline)
        .arg(&recording);
    if verb == "forecast" {
        command.args([
            "--at-ms",
            &at_ms.to_string(),
            "--horizon-ms",
            &horizon_ms.to_string(),
        ]);
    } else {
        command.arg("--json");
    }
    let output = command.assert().success();
    let value: Value = serde_json::from_slice(&output.get_output().stdout).expect("result JSON");
    if verb == "compare" {
        value
            .get("recordings")
            .and_then(Value::as_object)
            .and_then(|rows| rows.values().next())
            .expect("replay rows")
            .clone()
    } else {
        value
    }
}

fn signal(at_ms: u64, kind: &str, id: &str, source: &str) -> Value {
    json!({"at_ms":at_ms,"type":kind,"fields":{"id":id,"source":source}})
}

#[rstest::rstest]
#[case("ADCA", "Faithbound Kirin")]
#[case("AD2B", "Omega, the One")]
#[case("ACBD", "Kam'lanaut")]
#[case("AD51", "Eald'narche")]
fn finish_cancels_and_each_boss_can_restart(#[case] entry: &str, #[case] source: &str) {
    let rows = run(
        "compare",
        vec![
            signal(0, "Ability", entry, source),
            json!({"at_ms":30000,"type":"InCombat","fields":{"inGameCombat":"0"}}),
            signal(100000, "Ability", entry, source),
        ],
        120000,
        0,
    );
    let rows = rows.as_array().expect("rows");
    assert_eq!(rows.iter().filter(|r| r["action"] == "phase").count(), 2);
    assert!(!rows.iter().any(|r| {
        ["ambiguous", "unresolved", "violation"].contains(&r["action"].as_str().expect("action"))
    }));
    assert!(
        !rows.iter().any(|r| r["action"] == "event"
            && (30000..100000).contains(&r["at_ms"].as_u64().expect("time")))
    );
}

#[rstest::rstest]
#[case("B07B", "Quake", 98000)]
#[case("B079", "Stonega III", 98000)]
fn shijin_choice_predicts_before_resolution_and_common_arms_restores_late_schedule(
    #[case] choice: &str,
    #[case] name: &str,
    #[case] impact: u64,
) {
    let start = signal(0, "Ability", "ADCA", "Faithbound Kirin");
    let select = signal(95000, "StartsUsing", choice, "Faithbound Kirin");
    let result = run("forecast", vec![start.clone(), select.clone()], 96000, 4000);
    assert!(
        result["events"]
            .as_array()
            .expect("events")
            .iter()
            .any(|e| e["name"] == name && e["at_ms"] == impact)
    );
    // A shared observed continuation is enough; no future Wringer observation is supplied.
    let result = run(
        "forecast",
        vec![
            start,
            select,
            signal(123000, "Ability", "AD91", "Faithbound Kirin"),
        ],
        126500,
        6000,
    );
    assert!(
        result["events"]
            .as_array()
            .expect("events")
            .iter()
            .any(|e| e["name"] == "Wringer" && e["at_ms"] == 131500)
    );
    assert_eq!(result["diagnostics"], json!([]));
}

#[test]
fn missing_choice_is_reported_but_common_continuation_recovers() {
    let result = run(
        "forecast",
        vec![
            signal(0, "Ability", "ADCA", "Faithbound Kirin"),
            signal(123000, "Ability", "AD91", "Faithbound Kirin"),
        ],
        126500,
        6000,
    );
    assert!(
        result["diagnostics"]
            .as_array()
            .expect("diagnostics")
            .iter()
            .any(|r| r["action"] == "unresolved")
    );
    assert!(
        result["events"]
            .as_array()
            .expect("events")
            .iter()
            .any(|e| e["name"] == "Wringer" && e["at_ms"] == 131500)
    );
}

#[test]
fn reused_uranos_does_not_reenter_and_late_sync_restores_after_omission() {
    let start = signal(0, "Ability", "AD51", "Eald'narche");
    let rows = run(
        "compare",
        vec![
            start.clone(),
            signal(257700, "Ability", "AD51", "Eald'narche"),
        ],
        270000,
        0,
    );
    assert_eq!(
        rows.as_array()
            .expect("rows")
            .iter()
            .filter(|r| r["action"] == "phase")
            .count(),
        1
    );
    // Omit AD59; the second Phase Shift can still restore +7s progression without knowing Duplicate's future signal.
    for late in [false, true] {
        let observations = if late {
            vec![
                start.clone(),
                signal(174800, "Ability", "AD5A", "Eald'narche"),
            ]
        } else {
            vec![start.clone()]
        };
        let result = run("forecast", observations, 182000, 6500);
        let recovered = result["events"]
            .as_array()
            .expect("events")
            .iter()
            .any(|e| e["name"] == "Duplicate" && e["at_ms"] == 187000);
        assert_eq!(recovered, late);
    }
}

#[test]
fn duo_forecasts_simultaneous_abilities_without_cross_boss_sync() {
    let result = run(
        "forecast",
        vec![
            signal(0, "Ability", "AD2B", "Omega, the One"),
            signal(35000, "Ability", "AD06", "Ultima, the Feared"),
        ],
        107000,
        6000,
    );
    let events = result["events"].as_array().expect("events");
    assert!(
        events
            .iter()
            .any(|e| e["name"] == "Omega: Multi-Missile" && e["at_ms"] == 111800)
    );
    assert!(
        events
            .iter()
            .any(|e| e["name"] == "Ultima: Citadel Siege" && e["at_ms"] == 111900)
    );
}
