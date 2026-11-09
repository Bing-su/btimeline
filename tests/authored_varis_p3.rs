use std::fs;

use assert_cmd::cargo::cargo_bin_cmd;
use serde_json::{Value, json};

fn query(observations: Value, at: i64) -> Value {
    let directory = tempfile::tempdir().expect("temporary directory");
    let timeline = directory.path().join("varis.kdl");
    let recording = directory.path().join("recording.json");
    fs::write(&timeline, include_str!("../timelines/varis-ex-p3.kdl")).expect("standalone KDL");
    fs::write(
        &recording,
        serde_json::to_vec(&json!({"version":1,"end_ms":500000,"observations":observations}))
            .expect("encode recording"),
    )
    .expect("recording");
    let output = cargo_bin_cmd!("btimeline")
        .current_dir(directory.path())
        .arg("forecast")
        .arg(timeline)
        .arg(recording)
        .arg("--at-ms")
        .arg(at.to_string())
        .args(["--horizon-ms", "6500"])
        .assert()
        .success();
    let forecast: Value =
        serde_json::from_slice(&output.get_output().stdout).expect("forecast JSON");
    assert_eq!(forecast.get("diagnostics"), Some(&json!([])));
    assert_eq!(forecast.get("projection_diagnostics"), Some(&json!([])));
    forecast
}

fn block(letter: char) -> (&'static str, i64, &'static str, i64) {
    [
        ('L', ("4CD8", 53000, "Magitek Burst", 21400)),
        ('R', ("4CD9", 55000, "Altius", 14100)),
        ('E', ("4CD7", 58700, "Reinforcements", 10100)),
    ]
    .into_iter()
    .find_map(|(key, value)| (key == letter).then_some(value))
    .expect("known test block")
}

#[rstest::rstest]
#[case("LRE")]
#[case("LER")]
#[case("RLE")]
#[case("REL")]
#[case("ELR")]
#[case("ERL")]
fn all_six_orders_forecast_only_the_selected_history_and_join(#[case] order: &str) {
    for scale in [80, 100, 120] {
        let mut observations =
            vec![json!({"at_ms":0,"type":"Ability","fields":{"id":"4CCF","source":"Bladesblood"}})];
        let mut at = 16300;
        let mut targets = vec![];
        for (index, letter) in order.chars().enumerate() {
            let (id, duration, name, offset) = block(letter);
            observations.push(json!({"at_ms":at,"type":"StartsUsing","fields":{"id":id,"source":"Varis Yae Galvus"}}));
            let phase = if index == 2 {
                format!("final-{letter}")
            } else {
                order.chars().take(index + 1).collect()
            };
            targets.push((at + offset, name, phase));
            at += duration * scale / 100;
        }
        let join = at + 10000;
        observations.push(json!({"at_ms":join,"type":"Ability","fields":{"id":"4CE0","source":"Varis Yae Galvus"}}));
        for (target_at, name, phase) in targets {
            // Only choice signals are supplied: follow-up mechanics must be predicted before their own observations.
            let forecast = query(json!(observations), target_at - 5000);
            assert_eq!(
                forecast["phase"],
                format!("{phase}/main"),
                "{order} at {scale}%"
            );
            let events = forecast["events"].as_array().expect("events");
            assert!(
                events
                    .iter()
                    .any(|event| event["name"] == name && event["at_ms"] == target_at)
            );
            assert!(events.iter().all(|event| {
                event["id"]
                    .as_str()
                    .expect("event ID")
                    .starts_with(&format!("{phase}/main/"))
            }));
        }
        let forecast = query(json!(observations), join + 5500);
        assert_eq!(forecast["phase"], "joined/main");
        assert!(
            forecast["events"]
                .as_array()
                .expect("events")
                .iter()
                .any(|event| event["id"] == "joined/main/joined-first"
                    && event["at_ms"] == join + 10500)
        );
    }
}

#[rstest::rstest]
#[case(false)]
#[case(true)]
fn missing_or_reused_choice_cannot_invent_history_but_shared_marker_recovers(#[case] reused: bool) {
    let mut observations = vec![
        json!({"at_ms":0,"type":"Ability","fields":{"id":"4CCF","source":"Bladesblood"}}),
        json!({"at_ms":16300,"type":"StartsUsing","fields":{"id":"4CD8","source":"Varis Yae Galvus"}}),
    ];
    if reused {
        observations.push(json!({"at_ms":69300,"type":"StartsUsing","fields":{"id":"4CD9","source":"Varis Yae Galvus"}}));
    }
    observations.push(json!({"at_ms":124300,"type":"StartsUsing","fields":{"id":if reused {"4CD8"} else {"4CD7"},"source":"Varis Yae Galvus"}}));
    observations.push(
        json!({"at_ms":200000,"type":"Ability","fields":{"id":"4CE0","source":"Varis Yae Galvus"}}),
    );
    assert_eq!(
        query(json!(observations), 125000)["phase"],
        if reused { "LR/main" } else { "L/main" }
    );
    assert_eq!(query(json!(observations), 205500)["phase"], "joined/main");
}

#[test]
fn p1_choices_do_not_start_the_p3_excerpt() {
    let forecast = query(
        json!([
            {"at_ms":0,"type":"StartsUsing","fields":{"id":"4CD7","source":"Varis Yae Galvus"}},
            {"at_ms":1000,"type":"Ability","fields":{"id":"4CE0","source":"Varis Yae Galvus"}}
        ]),
        2000,
    );
    assert_eq!(forecast["status"], "idle");
    assert_eq!(forecast["events"], json!([]));
}
