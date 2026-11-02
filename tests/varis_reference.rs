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

#[test]
fn varis_permutations_keep_unrepresented_followups_visible() {
    let dir = tempfile::tempdir().unwrap();
    let mut paths = Vec::new();
    for (peer, order) in ORDERS.iter().enumerate() {
        // Synthetic P3 excerpts isolate history-dependent order; production never reads cactbot.
        let mut data: Value = serde_json::from_str(include_str!("fixtures/pull.json")).unwrap();
        let code = format!("varis-reference-{peer}");
        let mut events = vec![
            json!({"timestamp":2000,"type":"cast","sourceID":10,"abilityGameID":0x4CCF,"fight":1}),
        ];
        let mut at = 16200;
        for choice in order {
            let (_, id, delay) = PHASES[*choice];
            events.push(json!({"timestamp":at,"type":"cast","sourceID":10,"abilityGameID":i64::from_str_radix(id,16).unwrap(),"fight":1}));
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
                ]
                .into_iter()
                .map(|(id, name)| json!({"gameID":id,"name":name,"type":"1"}))
                .collect(),
            ),
            ("/collection/eventCount", events.len().into()),
            ("/events", events.into()),
        ] {
            *data.pointer_mut(pointer).unwrap() = value;
        }
        let path = dir.path().join(format!("{peer}.json"));
        fs::write(&path, serde_json::to_vec(&data).unwrap()).unwrap();
        paths.push(path);
    }
    let output = dir.path().join("generated.kdl");
    cargo_bin_cmd!("btimeline")
        .arg("generate")
        .args(paths)
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
        assert_eq!(trace["nodes"].as_array().unwrap().len(), 5);
    }
    assert!(
        evidence["alignment_diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(
                |row| row["reason"] == "no forced correspondence or supported branch"
                    && row["unrepresented"]
                        .as_array()
                        .is_some_and(|rows| !rows.is_empty())
            ),
        "Unsupported followup choices must remain visible in the omission diagnostics"
    );
}
