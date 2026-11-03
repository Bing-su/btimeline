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
            signal(119500, "StartsUsing", "AD91", "Faithbound Kirin"),
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
            signal(119500, "StartsUsing", "AD91", "Faithbound Kirin"),
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

#[rstest::rstest]
#[case("StartsUsing", "B07B", 95000, 123500)]
#[case("StartsUsing", "B079", 101000, 121500)]
#[case("StartsUsing", "AD84", 78000, 122700)]
#[case("Ability", "AD82", 90000, 124600)]
#[case("Ability", "ADCD", 91300, 124600)]
#[case("StartsUsing", "A916", 106100, 124600)]
fn each_shijin_route_predicts_common_arms_before_its_own_signal(
    #[case] kind: &str,
    #[case] id: &str,
    #[case] select: u64,
    #[case] arms: u64,
) {
    let mut observations = vec![
        signal(0, "Ability", "ADCA", "Faithbound Kirin"),
        signal(
            select,
            kind,
            id,
            if id == "AD84" {
                "Moonbound Genbu"
            } else {
                "Faithbound Kirin"
            },
        ),
    ];
    if id == "AD82" {
        observations.push(signal(91300, "Ability", "ADCD", "Faithbound Kirin"));
        // Riddle also occurs on the longer Eastwind route; it must not override the earlier choice.
        observations.push(signal(
            select + 2900,
            "StartsUsing",
            "AFF5",
            "Faithbound Kirin",
        ));
        observations.push(signal(106100, "StartsUsing", "A916", "Faithbound Kirin"));
    }
    let result = run("forecast", observations, arms - 5000, 6500);
    assert_eq!(result["diagnostics"], json!([]));
    assert_eq!(result["projection_diagnostics"], json!([]));
    assert!(
        result["events"]
            .as_array()
            .expect("events")
            .iter()
            .any(|e| e["name"] == "Wrought Arms" && e["at_ms"] == arms)
    );
}

#[test]
fn early_kirin_marker_only_selects_within_its_window() {
    for (at, expected) in [(91300, true), (104500, false)] {
        let result = run(
            "forecast",
            vec![
                signal(0, "Ability", "ADCA", "Faithbound Kirin"),
                signal(at, "Ability", "ADCD", "Faithbound Kirin"),
            ],
            at,
            7000,
        );
        let expected_id = "kirin-early-recovery-riddle";
        let events = result["events"].as_array().expect("events");
        assert_eq!(
            events.iter().any(|event| event["id"] == expected_id),
            expected
        );
    }
}

#[test]
fn double_cast_recovers_the_paired_forecast_without_the_wheel_signal() {
    let result = run(
        "forecast",
        vec![
            signal(0, "Ability", "ADCA", "Faithbound Kirin"),
            signal(106100, "StartsUsing", "A916", "Faithbound Kirin"),
        ],
        112200,
        6500,
    );
    assert_eq!(result["diagnostics"], json!([]));
    assert_eq!(result["projection_diagnostics"], json!([]));
    let events = result["events"].as_array().expect("events");
    for id in ["kirin-recovery-quake", "kirin-recovery-stonega"] {
        assert!(
            events
                .iter()
                .any(|event| event["id"] == id && event["at_ms"] == 117200)
        );
    }
}

#[test]
fn first_sling_resumes_eald_and_later_sling_preserves_the_clock() {
    let source = "Eald'narche";
    let rows = run(
        "compare",
        vec![
            signal(21007, "Ability", "AD4A", source),
            signal(54807, "Ability", "AD4C", source),
        ],
        70000,
        0,
    );
    let rows = rows.as_array().expect("replay rows");
    assert_eq!(
        rows.iter().filter(|row| row["action"] == "phase").count(),
        1
    );
    assert!(
        rows.iter()
            .any(|row| row["action"] == "skipped" && row["detail"] == "eald-1")
    );
    assert!(
        rows.iter()
            .any(|row| row["event"]["id"] == "eald-2" && row["at_ms"] == 21007)
    );
    assert!(
        rows.iter()
            .any(|row| row["event"]["id"] == "eald-3" && row["at_ms"] == 37707)
    );
    assert!(
        rows.iter()
            .any(|row| row["event"]["id"] == "eald-4" && row["at_ms"] == 54807)
    );
}

#[test]
fn early_scrivening_cast_recovers_before_its_five_second_query() {
    let result = run(
        "forecast",
        vec![
            signal(0, "Ability", "ACBD", "Kam'lanaut"),
            signal(104700, "StartsUsing", "ACB2", "Kam'lanaut"),
        ],
        105700,
        6500,
    );
    assert!(
        result["events"]
            .as_array()
            .expect("events")
            .iter()
            .any(|e| e["name"] == "Esoteric Scrivening" && e["at_ms"] == 110700)
    );
    let result = run(
        "forecast",
        vec![signal(0, "Ability", "ACBD", "Kam'lanaut")],
        318700,
        6500,
    );
    assert!(
        result["events"]
            .as_array()
            .expect("events")
            .iter()
            .any(|e| e["name"] == "Crystalline Resonance" && e["at_ms"] == 323700)
    );
}

#[rstest::rstest]
#[case("Omega, the One", true)]
#[case("Ultima, the Feared", true)]
#[case("Player", false)]
fn joint_schedule_cancels_only_on_a_named_boss_defeat(#[case] target: &str, #[case] cancels: bool) {
    let result = run(
        "forecast",
        vec![
            signal(0, "Ability", "AD2B", "Omega, the One"),
            json!({"at_ms":90000,"type":"WasDefeated","fields":{"target":target}}),
        ],
        96000,
        6500,
    );
    assert_eq!(
        result["events"].as_array().expect("events").is_empty(),
        cancels
    );
}

#[rstest::rstest]
#[case(
    "AD09",
    5153,
    "ultima-solo-mana-screen",
    "AD08",
    10261,
    "ultima-solo-energy-orb"
)]
#[case(
    "AD08",
    4365,
    "ultima-solo-energy-orb",
    "AD09",
    9813,
    "ultima-solo-mana-screen"
)]
fn ultima_solo_forecasts_only_observed_new_casts(
    #[case] first_id: &str,
    #[case] first_start: u64,
    #[case] first_event: &str,
    #[case] second_id: &str,
    #[case] second_start: u64,
    #[case] second_event: &str,
) {
    let boundary = 90000;
    let ultima = "Ultima, the Feared";
    let mut observations = vec![
        signal(0, "Ability", "AD2B", "Omega, the One"),
        signal(boundary - 357, "StartsUsing", "AD08", ultima),
        json!({"at_ms":boundary,"type":"WasDefeated","fields":{"target":"Omega, the One"}}),
        signal(boundary + 2628, "Ability", "AD08", ultima),
    ];
    // The cast already in progress has no newly observed start after the handoff.
    let waiting = run("forecast", observations.clone(), boundary + 2700, 5000);
    assert_eq!(waiting["phase"], "ultima-solo");
    assert_eq!(waiting["events"], json!([]));
    observations.push(signal(
        boundary + first_start,
        "StartsUsing",
        first_id,
        ultima,
    ));
    observations.push(signal(
        boundary + second_start,
        "StartsUsing",
        second_id,
        ultima,
    ));
    observations.push(json!({
        "at_ms":boundary + second_start + 100,
        "type":"WasDefeated",
        "fields":{"target":"Omega, the One"}
    }));
    for (at, id) in [(first_start, first_event), (second_start, second_event)] {
        let forecast = run("forecast", observations.clone(), boundary + at + 500, 7000);
        let events = forecast["events"].as_array().unwrap();
        assert_eq!(events.len(), 1, "{forecast}");
        assert_eq!(events[0]["id"], id);
        assert_eq!(events[0]["at_ms"], boundary + at + 3000);
    }
    observations.push(json!({
        "at_ms":boundary + second_start + 1000,
        "type":"WasDefeated",
        "fields":{"target":ultima}
    }));
    assert_eq!(
        run(
            "forecast",
            observations,
            boundary + second_start + 1500,
            4000
        )["events"],
        json!([])
    );
}

#[test]
fn ultima_can_forecast_two_consecutive_mana_screens_in_one_phase() {
    let observations = vec![
        signal(0, "Ability", "AD2B", "Omega, the One"),
        json!({"at_ms":90000,"type":"WasDefeated","fields":{"target":"Omega, the One"}}),
        signal(95000, "StartsUsing", "AD09", "Ultima, the Feared"),
        signal(100000, "StartsUsing", "AD09", "Ultima, the Feared"),
    ];
    let forecast = run("forecast", observations.clone(), 100000, 4000);
    assert_eq!(forecast["events"][0]["id"], "ultima-solo-mana-screen");
    assert_eq!(forecast["events"][0]["at_ms"], 103000);
    assert_eq!(forecast["events"][0]["context"]["sequence_instance"], 2);
    let rows = run("compare", observations, 100000, 4000);
    let executions = rows
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["event"]["id"] == "ultima-solo-mana-screen")
        .map(|row| {
            (
                &row["at_ms"],
                &row["context"]["phase"],
                &row["context"]["sequence_instance"],
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        executions,
        [
            (&json!(98000), &json!("ultima-solo"), &json!(1)),
            (&json!(103000), &json!("ultima-solo"), &json!(2)),
        ]
    );
}

#[test]
fn ultima_first_defeat_arms_omega_cast_and_cancels_on_omega_defeat() {
    let mut observations = vec![
        signal(0, "Ability", "AD2B", "Omega, the One"),
        json!({"at_ms":90000,"type":"WasDefeated","fields":{"target":"Ultima, the Feared"}}),
        signal(95000, "StartsUsing", "AD09", "Ultima, the Feared"),
        signal(95159, "StartsUsing", "AD23", "Omega, the One"),
        json!({"at_ms":95259,"type":"WasDefeated","fields":{"target":"Ultima, the Feared"}}),
    ];
    let waiting = run("forecast", observations.clone(), 95000, 5000);
    assert_eq!(waiting["phase"], "omega-solo");
    assert_eq!(waiting["events"], json!([]));
    let forecast = run("forecast", observations.clone(), 95659, 5000);
    let events = forecast["events"].as_array().unwrap();
    assert_eq!(events.len(), 1, "{forecast}");
    assert_eq!(events[0]["id"], "omega-solo-trajectory-projection");
    assert_eq!(events[0]["at_ms"], 98659);
    observations.push(json!({
        "at_ms":97000,
        "type":"WasDefeated",
        "fields":{"target":"Omega, the One"}
    }));
    assert_eq!(
        run("forecast", observations, 97500, 5000)["events"],
        json!([])
    );
}
