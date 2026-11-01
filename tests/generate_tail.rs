use std::fs;
use std::path::{Path, PathBuf};

use assert_cmd::cargo::cargo_bin_cmd;
use proptest::prelude::*;
use serde_json::{Value, json};

fn input(directory: &Path, index: usize, problem: &str, base: i64, shift: i64) -> PathBuf {
    let mut data: Value =
        serde_json::from_str(include_str!("fixtures/pull.json")).expect("fixture");
    let kill = index < 2;
    let end = if kill && problem != "silent-clear" {
        7000
    } else if kill {
        11000
    } else {
        13000
    };
    let mut records = vec![(2000, 0), (4000, 1), (6000, 2)];
    if !kill {
        records.extend([(8000, 3), (10000, 3), (12000, 4)]);
    }
    if index == 3 {
        match problem {
            "one-support" => records.truncate(3),
            "missing-anchor" => records.retain(|(_, id)| *id != 2),
            "internal-gap" => records.retain(|(at, _)| *at != 8000),
            "reversed-anchor" => {
                records = vec![
                    (2000, 2),
                    (4000, 0),
                    (6000, 1),
                    (8000, 3),
                    (10000, 3),
                    (12000, 4),
                ]
            }
            "unordered-anchor" => records.push((6000, 5)),
            _ => (),
        }
    }
    if problem == "nearby-waves" && !kill {
        records = vec![
            (2000, 0),
            (4000, 1),
            (6000, 2),
            (8000, 3),
            (8050, 3),
            (12000, 4),
        ];
    }
    if problem == "phase" && index == 3 {
        data.pointer_mut("/report/fights/0")
            .expect("fight")
            .as_object_mut()
            .expect("fight object")
            .insert(
                "phaseTransitions".into(),
                json!([{"id":2,"startTime":9000+shift}]),
            );
    }
    let code = format!("tail{index}");
    for (pointer,value) in [
        ("/report/code",code.clone().into()),
        ("/collection/reportCode",code.into()),
        ("/report/fights/0/kill",kill.into()),
        ("/report/fights/0/endTime",(end+shift).into()),
        ("/collection/endTime",(end+shift).into()),
        ("/report/masterData/abilities",(0..6).map(|id|json!({"gameID":base+id,"name":format!("Spell{id}"),"type":"1"})).collect()),
        ("/events",records.iter().map(|(at,id)|json!({"timestamp":at+shift,"type":"cast","sourceID":10,"abilityGameID":base+id,"fight":1})).collect()),
        ("/collection/eventCount",records.len().into()),
    ] {
        *data.pointer_mut(pointer).expect("fixture field") = value;
    }
    let path = directory.join(format!("{index}.json"));
    fs::write(&path, serde_json::to_vec(&data).expect("serialize fixture")).expect("write fixture");
    path
}

fn generate(paths: &[PathBuf], mode: &str, output: &Path) -> Value {
    cargo_bin_cmd!("btimeline")
        .arg("generate")
        .args(paths)
        .args(["--mode", mode, "-o"])
        .arg(output)
        .assert()
        .success();
    serde_json::from_slice(
        &fs::read(output.with_extension("evidence.json")).expect("read evidence"),
    )
    .expect("evidence")
}

#[rstest::rstest]
#[case("normal", 2, 1)]
#[case("silent-clear", 0, 1)]
#[case("one-support", 0, 0)]
#[case("missing-anchor", 0, 0)]
#[case("internal-gap", 0, 0)]
#[case("nearby-waves", 0, 1)]
#[case("phase", 0, 0)]
#[case("reversed-anchor", 0, 0)]
#[case("unordered-anchor", 0, 0)]
fn tail_learning_requires_supported_correspondence_and_genuine_censoring(
    #[case] problem: &str,
    #[case] repeats: usize,
    #[case] ending: usize,
) {
    let dir = tempfile::tempdir().unwrap();
    let paths: Vec<_> = (0..4)
        .map(|index| input(dir.path(), index, problem, 100, 0))
        .collect();
    let output = dir.path().join("timeline.kdl");
    let evidence = generate(&paths, "raid", &output);
    let events = evidence["events"].as_array().unwrap();
    for (id, expected) in [(103, repeats), (104, ending)] {
        let selected: Vec<_> = events
            .iter()
            .filter(|event| event["activity"]["identifier"]["Ability"] == id)
            .collect();
        assert_eq!(selected.len(), expected, "{problem}: {id}");
        for event in selected {
            assert_eq!(event["alignment"]["method"], "censored-tail-v1");
            assert_eq!(event["samples"].as_array().unwrap().len(), 2);
            assert!(
                event["samples"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|sample| matches!(sample["trace"].as_str(), Some("tail2/1" | "tail3/1")))
            );
        }
    }
    assert_eq!(evidence["analysis"]["inputs"][2]["kill"], false);
    let reverse = dir.path().join("reverse.kdl");
    generate(
        &paths.iter().rev().cloned().collect::<Vec<_>>(),
        "raid",
        &reverse,
    );
    assert_eq!(fs::read(&output).unwrap(), fs::read(&reverse).unwrap());
    if problem == "normal" {
        insta::assert_snapshot!(
            "generated_censored_tail",
            fs::read_to_string(&output).unwrap()
        );
        let checks = evidence["alignment_diagnostics"].as_array().unwrap();
        assert!(
            checks
                .iter()
                .filter(|row| row["method"] == "censored-reference-prefix-v1")
                .all(|row| row["training"] == true)
        );
        // Runtime receives only the schedule; observed fight ends never become invented ACT signals.
        fs::remove_file(output.with_extension("evidence.json")).unwrap();
        let recording = dir.path().join("recording.json");
        fs::write(
            &recording,
            serde_json::to_vec(&json!({"version":1,"end_ms":14000,"observations":[
                {"at_ms":0,"type":"InCombat","fields":{"inGameCombat":"1"}}
            ]}))
            .unwrap(),
        )
        .unwrap();
        let replay = cargo_bin_cmd!("btimeline")
            .arg("compare")
            .arg(&output)
            .arg(&recording)
            .arg("--json")
            .assert()
            .success();
        let result: Value = serde_json::from_slice(&replay.get_output().stdout).unwrap();
        assert_eq!(
            result["recordings"][recording.to_str().unwrap()]
                .as_array()
                .unwrap()
                .iter()
                .filter(|row| row["action"] == "event")
                .count(),
            6
        );
    }
}

#[rstest::rstest]
#[case("raid", 6)]
#[case("dungeon", 3)]
#[case("alliance", 3)]
fn new_incomplete_samples_need_a_known_clock_origin(#[case] mode: &str, #[case] count: usize) {
    let dir = tempfile::tempdir().unwrap();
    let paths: Vec<_> = (0..4)
        .map(|index| input(dir.path(), index, "normal", 200, 0))
        .collect();
    let output = dir.path().join("timeline.kdl");
    let evidence = generate(&paths, mode, &output);
    assert_eq!(evidence["events"].as_array().unwrap().len(), count);
}

#[test]
fn validation_never_trains_on_the_selected_clear() {
    let dir = tempfile::tempdir().unwrap();
    let mut paths: Vec<_> = (0..4)
        .map(|index| input(dir.path(), index, "normal", 300, 0))
        .collect();
    let mut held: Value = serde_json::from_slice(&fs::read(&paths[0]).unwrap()).unwrap();
    held["report"]["code"] = "zzheld".into();
    held["collection"]["reportCode"] = "zzheld".into();
    held["report"]["fights"][0]["endTime"] = 14000.into();
    held["collection"]["endTime"] = 14000.into();
    held["events"]
        .as_array_mut()
        .unwrap()
        .push(json!({"timestamp":12000,"type":"cast","sourceID":10,"abilityGameID":305,"fight":1}));
    held["collection"]["eventCount"] = 4.into();
    let path = dir.path().join("held.json");
    fs::write(&path, serde_json::to_vec(&held).unwrap()).unwrap();
    paths.push(path);
    let evidence = generate(&paths, "raid", &dir.path().join("timeline.kdl"));
    let validation = evidence["alignment_diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["method"] == "automatic-validation-v1")
        .unwrap();
    assert_eq!(validation["split"]["held_out"], "zzheld/1");
    let model = validation["model_events"].as_array().unwrap();
    assert!(
        model
            .iter()
            .any(|event| event["activity"]["identifier"]["Ability"] == 304)
    );
    assert!(
        model
            .iter()
            .all(|event| event["samples"].get("zzheld/1").is_none())
    );
    assert!(
        model
            .iter()
            .all(|event| event["activity"]["identifier"]["Ability"] != 305)
    );
    let reversed = generate(
        &paths.iter().rev().cloned().collect::<Vec<_>>(),
        "raid",
        &dir.path().join("reversed.kdl"),
    );
    let repeated = reversed["alignment_diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["method"] == "automatic-validation-v1")
        .unwrap();
    assert_eq!(validation, repeated);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(12))]
    #[test]
    fn tail_recovery_is_independent_of_ability_ids_and_time_origin(base in 1_i64..60000, shift in 0_i64..2000) {
        let dir = tempfile::tempdir().unwrap();
        let paths:Vec<_> = (0..4).map(|index|input(dir.path(),index,"normal",base,shift)).collect();
        let evidence = generate(&paths,"raid",&dir.path().join("timeline.kdl"));
        let events = evidence["events"].as_array().unwrap();
        prop_assert_eq!(events.len(),6);
        let ending = events.iter().find(|event|event["activity"]["identifier"]["Ability"]==base+4).unwrap();
        prop_assert_eq!(ending["median_offset_ms"].as_i64().unwrap(),11000+shift);
    }
}
