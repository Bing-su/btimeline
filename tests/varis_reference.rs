use std::fs;

use assert_cmd::cargo::cargo_bin_cmd;
use serde_json::{Value, json};

const ORDERS: [[usize; 3]; 6] = [
    [0, 1, 2],
    [0, 2, 1],
    [1, 0, 2],
    [1, 2, 0],
    [2, 0, 1],
    [2, 1, 0],
];
const PHASES: [(&str, &str, i64); 3] = [
    ("loaded", "4CD8", 54300),
    ("reinforced", "4CD9", 52800),
    ("electrified", "4CD7", 58700),
];

#[rstest::rstest]
#[case(ORDERS[0])]
#[case(ORDERS[1])]
#[case(ORDERS[2])]
#[case(ORDERS[3])]
#[case(ORDERS[4])]
#[case(ORDERS[5])]
fn previous_gunshield_controls_the_next_window(#[case] order: [usize; 3]) {
    let dir = tempfile::tempdir().unwrap();
    let timeline = dir.path().join("timeline.kdl");
    fs::write(
        &timeline,
        include_str!("fixtures/timeline/varis-choice-windows.kdl"),
    )
    .unwrap();
    let mut observations = vec![json!({"at_ms":0,"type":"InCombat","fields":{"inGameCombat":"1"}})];
    let mut at = 14200;
    let mut expected = vec![(0, "random-groups")];
    for (index, choice) in order.iter().enumerate() {
        let (phase, id, _) = PHASES[*choice];
        if index > 0 {
            at += PHASES[order[index - 1]].2;
            // Same signal just outside the previous phase's window must not advance the fight.
            observations.push(json!({"at_ms":at-10100,"type":"StartsUsing","fields":{"id":id,"source":"Varis Yae Galvus"}}));
        }
        observations.push(
            json!({"at_ms":at,"type":"StartsUsing","fields":{"id":id,"source":"Varis Yae Galvus"}}),
        );
        expected.push((at, phase));
    }
    let recording = dir.path().join("recording.json");
    fs::write(
        &recording,
        serde_json::to_vec(&json!({"version":1,"end_ms":at+5100,"observations":observations}))
            .unwrap(),
    )
    .unwrap();
    let result = cargo_bin_cmd!("btimeline")
        .arg("compare")
        .arg(&timeline)
        .arg(&recording)
        .arg("--json")
        .assert()
        .success();
    let data: Value = serde_json::from_slice(&result.get_output().stdout).unwrap();
    let rows = data["recordings"][recording.to_str().unwrap()]
        .as_array()
        .unwrap();
    let phases: Vec<_> = rows
        .iter()
        .filter(|row| row["action"] == "phase")
        .map(|row| {
            (
                row["at_ms"].as_i64().unwrap(),
                row["detail"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(phases, expected);
    assert_eq!(
        rows.iter().filter(|row| row["action"] == "event").count(),
        3
    );
    assert!(!rows.iter().any(|row| {
        ["ambiguous", "unresolved", "violation", "skipped"]
            .contains(&row["action"].as_str().unwrap())
    }));
}

fn permutation_inputs(directory: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut paths = Vec::new();
    for (peer, order) in ORDERS.iter().enumerate() {
        // Synthetic P3 excerpts isolate history-dependent order; production never reads cactbot.
        let mut data: Value =
            serde_json::from_str(include_str!("fixtures/pull.json")).expect("fixture");
        let code = format!("varis-reference-{peer}");
        let mut events = vec![
            json!({"timestamp":2000,"type":"cast","sourceID":10,"abilityGameID":0x4CCF,"fight":1}),
        ];
        let mut at = 16200;
        for choice in order {
            let (_, id, delay) = PHASES.get(*choice).expect("block");
            events.push(json!({"timestamp":at,"type":"cast","sourceID":10,"abilityGameID":i64::from_str_radix(id,16).expect("ability"),"fight":1}));
            let follow = [0x4CDF, 0x4CDB, 0x4CDA]
                .get(*choice)
                .copied()
                .expect("followup");
            events.push(json!({"timestamp":at+20000,"type":"cast","sourceID":10,"abilityGameID":follow,"fight":1}));
            // A shared action has different encounter-wide ordinals when the blocks change order.
            events.push(json!({"timestamp":at+30000,"type":"cast","sourceID":10,"abilityGameID":0x4CF0,"fight":1}));
            at += delay;
        }
        events.push(json!({"timestamp":200000,"type":"cast","sourceID":10,"abilityGameID":0x4CE0,"fight":1}));
        for (pointer, value) in [
            ("/report/code", code.clone().into()),
            ("/collection/reportCode", code.into()),
            ("/report/endTime", 210000.into()),
            ("/report/fights/0/endTime", 210000.into()),
            ("/collection/endTime", 210000.into()),
            (
                "/report/masterData/abilities",
                vec![
                    (0x4CCF, "Vivere Militare Est"),
                    (0x4CD8, "Loaded Gunshield"),
                    (0x4CD9, "Reinforced Gunshield"),
                    (0x4CD7, "Electrified Gunshield"),
                    (0x4CE0, "Gunshield"),
                    (0x4CDF, "Magitek Burst"),
                    (0x4CDB, "Magitek Shielding"),
                    (0x4CDA, "Magitek Shock"),
                    (0x4CF0, "Citius"),
                ]
                .into_iter()
                .map(|(id, name)| json!({"gameID":id,"name":name,"type":"1"}))
                .collect(),
            ),
            ("/collection/eventCount", events.len().into()),
            ("/events", events.into()),
        ] {
            *data.pointer_mut(pointer).expect("fixture field") = value;
        }
        let path = directory.join(format!("{peer}.json"));
        fs::write(&path, serde_json::to_vec(&data).expect("encode fixture"))
            .expect("write fixture");
        paths.push(path);
    }
    paths
}

fn assert_history_summary_and_input_order(
    evidence: &Value,
    paths: &[std::path::PathBuf],
    output: &std::path::Path,
) {
    let history = evidence.pointer("/histories/0").expect("history");
    let shared: Vec<_> = evidence
        .get("events")
        .and_then(Value::as_array)
        .expect("events")
        .iter()
        .filter(|event| event.pointer("/activity/identifier/Ability") == Some(&json!(0x4CF0)))
        .collect();
    assert!(!shared.is_empty());
    assert!(shared.iter().all(|event| {
        event
            .get("event")
            .and_then(Value::as_str)
            .is_some_and(|id| id.ends_with("-01"))
    }));
    insta::assert_json_snapshot!(
        "generated_permutation_states",
        json!({
            "triggers":history.pointer("/model/blocks").and_then(Value::as_array).expect("blocks").iter().map(|block|block.pointer("/trigger/identifier").expect("identifier").clone()).collect::<Vec<_>>(),
            "states":history.get("states").and_then(Value::as_array).expect("states").len(),
            "edges":history.get("edges").and_then(Value::as_array).expect("edges").len(),
            "window_policy":history.get("window_policy").expect("window policy")
        })
    );
    let reversed = output.with_file_name("reversed.kdl");
    cargo_bin_cmd!("btimeline")
        .arg("generate")
        .args(paths.iter().rev())
        .args(["--mode", "raid", "-o"])
        .arg(&reversed)
        .assert()
        .success();
    assert_eq!(
        fs::read_to_string(output).expect("timeline"),
        fs::read_to_string(reversed).expect("reversed timeline")
    );
}

#[test]
fn generated_histories_predict_followups_without_the_evidence() {
    let dir = tempfile::tempdir().unwrap();
    let paths = permutation_inputs(dir.path());
    let output = dir.path().join("generated.kdl");
    cargo_bin_cmd!("btimeline")
        .arg("generate")
        .args(&paths)
        .args(["--mode", "raid", "-o"])
        .arg(&output)
        .assert()
        .success();
    let evidence: Value =
        serde_json::from_slice(&fs::read(output.with_extension("evidence.json")).unwrap()).unwrap();
    assert_eq!(
        evidence["progression"]["traces"].as_array().unwrap().len(),
        6
    );
    for trace in evidence["progression"]["traces"].as_array().unwrap() {
        assert_eq!(trace["nodes"].as_array().unwrap().len(), 11);
    }
    assert_eq!(evidence["histories"][0]["status"], "rendered");
    assert_history_summary_and_input_order(&evidence, &paths, &output);
    assert_eq!(
        evidence["histories"][0]["model"]["blocks"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    fs::remove_file(output.with_extension("evidence.json")).unwrap();
    for (order, percentage, missing) in ORDERS
        .into_iter()
        .chain([[0, 2, 0]])
        .flat_map(|order| [(order, 100, false), (order, 80, false), (order, 120, false)])
        .chain([([0, 1, 2], 100, true)])
    {
        let mut observations = vec![
            json!({"at_ms":0,"type":"InCombat","fields":{"inGameCombat":"1"}}),
            json!({"at_ms":500,"type":"Ability","fields":{"id":"4CD8","source":"Synthetic Boss"}}),
            json!({"at_ms":1000,"type":"Ability","fields":{"id":"4CCF","source":"Synthetic Boss"}}),
        ];
        let mut at = 15200;
        let mut expected = Vec::new();
        for (index, choice) in order.into_iter().enumerate() {
            let (_, id, delay) = PHASES[choice];
            if !(missing && index == 1) {
                observations.push(
                json!({"at_ms":at,"type":"Ability","fields":{"id":id,"source":"Synthetic Boss"}}),
            );
            }
            if (!missing || index == 0) && (expected.len() < 2 || order != [0, 2, 0]) {
                expected.push(at + 20000);
            }
            at += delay * percentage / 100;
        }
        let recording = dir.path().join("recording.json");
        fs::write(
            &recording,
            serde_json::to_vec(&json!({"version":1,"end_ms":200000,"observations":observations}))
                .unwrap(),
        )
        .unwrap();
        let result = cargo_bin_cmd!("btimeline")
            .arg("compare")
            .arg(&output)
            .arg(&recording)
            .arg("--json")
            .assert()
            .success();
        let data: Value = serde_json::from_slice(&result.get_output().stdout).unwrap();
        let rows = data["recordings"][recording.to_str().unwrap()]
            .as_array()
            .unwrap();
        let actual: Vec<_> = rows
            .iter()
            .filter(|row| {
                row["action"] == "event"
                    && ["Magitek Burst", "Magitek Shielding", "Magitek Shock"]
                        .iter()
                        .any(|name| {
                            row["detail"]
                                .as_str()
                                .unwrap()
                                .contains(&format!(": {name} ("))
                        })
            })
            .map(|row| row["at_ms"].as_i64().unwrap())
            .collect();
        assert_eq!(actual, expected, "{order:?}");
        assert!(!rows.iter().any(|row| {
            ["ambiguous", "unresolved", "violation", "skipped"]
                .contains(&row["action"].as_str().unwrap())
        }));
    }
}

#[test]
fn a_history_overlay_cannot_drop_previously_supported_displays() {
    let dir = tempfile::tempdir().unwrap();
    let paths = permutation_inputs(dir.path());
    for (peer, path) in paths.iter().enumerate() {
        let mut data: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        data["report"]["fights"][0]["kill"] = (peer < 2).into();
        if peer < 2 {
            let at = data["events"]
                .as_array()
                .unwrap()
                .iter()
                .find(|event| event["abilityGameID"] == 0x4CD8)
                .unwrap()["timestamp"]
                .as_i64()
                .unwrap()
                + 30000;
            data["events"].as_array_mut().unwrap().push(
                json!({"timestamp":at,"type":"cast","sourceID":10,"abilityGameID":5000,"fight":1}),
            );
            data["report"]["masterData"]["abilities"]
                .as_array_mut()
                .unwrap()
                .push(json!({"gameID":5000,"name":"Previously supported","type":"1"}));
            data["collection"]["eventCount"] = data["events"].as_array().unwrap().len().into();
        }
        fs::write(path, serde_json::to_vec(&data).unwrap()).unwrap();
    }
    let output = dir.path().join("timeline.kdl");
    cargo_bin_cmd!("btimeline")
        .arg("generate")
        .args(paths)
        .args(["--mode", "raid", "-o"])
        .arg(&output)
        .assert()
        .success();
    let evidence: Value =
        serde_json::from_slice(&fs::read(output.with_extension("evidence.json")).unwrap()).unwrap();
    assert_eq!(evidence["histories"][0]["status"], "unrendered");
    assert!(
        evidence["histories"][0]["reason"]
            .as_str()
            .unwrap()
            .contains("existing display")
    );
    assert!(
        evidence["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["activity"]["identifier"]["Ability"] == 5000)
    );
}
