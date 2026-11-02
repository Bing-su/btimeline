use std::fs;
use std::path::{Path, PathBuf};

use assert_cmd::cargo::cargo_bin_cmd;
use serde_json::{Value, json};

fn fixture(directory: &Path, code: &str, reverse: bool, starts: [i64; 2], prefix: bool) -> PathBuf {
    let mut data: Value =
        serde_json::from_str(include_str!("fixtures/pull.json")).expect("valid test fixture");
    for (pointer, value) in [
        ("/report/code", code.into()),
        ("/collection/reportCode", code.into()),
        ("/report/fights/0/kill", false.into()),
        ("/report/fights/0/endTime", 60000.into()),
        ("/report/endTime", 60000.into()),
        ("/collection/endTime", 60000.into()),
    ] {
        *data.pointer_mut(pointer).expect("fixture field") = value;
    }
    data.pointer_mut("/report/masterData/actors")
        .expect("fixture actors")
        .as_array_mut()
        .expect("valid test fixture")
        .push(json!({"id":11,"name":"Second Boss","gameID":99002,"type":"NPC","subType":"Boss"}));
    data.pointer_mut("/report/fights/0/enemyNPCs")
        .expect("fixture enemies")
        .as_array_mut()
        .expect("valid test fixture")
        .push(json!({"id":11,"gameID":99002}));
    *data
        .pointer_mut("/report/masterData/abilities")
        .expect("fixture abilities") = (1..=4)
        .map(|id| json!({"gameID":id,"name":format!("Skill {id}"),"type":"1"}))
        .collect();
    let mut events = Vec::new();
    for (boss, start) in starts.into_iter().enumerate() {
        let ids = if reverse ^ (boss == 1) {
            [1, 3, 2, 4]
        } else {
            [1, 2, 3, 4]
        };
        for (i, id) in ids
            .into_iter()
            .enumerate()
            .take(if prefix && boss == 0 { 2 } else { 4 })
        {
            // Fractional origins expose separate-rounding errors, e.g. 1080 + 1951 must become 2.0s.
            let offset = if i == 0 { 0 } else { i as i64 * 2000 - 49 };
            events.push(json!({"timestamp":1000+start+offset,"type":"cast","sourceID":10+boss,"abilityGameID":id,"fight":1}));
        }
        events.push(json!({"timestamp":1000+start+if prefix && boss==0 {3000} else {7000},"type":"death","targetID":10+boss,"fight":1}));
    }
    *data
        .pointer_mut("/collection/eventCount")
        .expect("fixture count") = events.len().into();
    *data.pointer_mut("/events").expect("fixture events") = events.into();
    let path = directory.join(format!("{code}.json"));
    fs::write(
        &path,
        serde_json::to_vec(&data).expect("valid test fixture"),
    )
    .expect("valid test fixture");
    path
}

fn generate(paths: &[PathBuf], timeline: &Path, mode: &str) -> Value {
    cargo_bin_cmd!("btimeline")
        .arg("generate")
        .args(paths)
        .args(["--mode", mode, "-o"])
        .arg(timeline)
        .assert()
        .success();
    serde_json::from_slice(
        &fs::read(timeline.with_extension("evidence.json")).expect("valid test fixture"),
    )
    .expect("valid test fixture")
}

#[rstest::rstest]
#[case("dungeon")]
#[case("alliance")]
fn boss_choices_use_local_clocks_and_predict_only_the_selected_boss(#[case] mode: &str) {
    let dir = tempfile::tempdir().expect("valid test fixture");
    let paths = [
        fixture(dir.path(), "alpha", false, [1080, 21111], false),
        fixture(dir.path(), "beta", true, [1111, 41080], false),
        fixture(dir.path(), "zeta", false, [3080, 31111], true),
    ];
    let timeline = dir.path().join("timeline.kdl");
    let evidence = generate(&paths, &timeline, mode);
    assert_eq!(evidence["generation_method"], "representative-route-v1");
    let routes = evidence["draft"]["routes"]
        .as_array()
        .expect("valid test fixture");
    assert_eq!(routes.len(), 2);
    assert!(routes.iter().all(|route| {
        route["choices"]
            .as_array()
            .expect("valid test fixture")
            .len()
            == 1
    }));
    let validation = evidence["alignment_diagnostics"]
        .as_array()
        .expect("valid test fixture")
        .iter()
        .find(|row| row["method"] == "automatic-draft-validation-v1")
        .expect("valid test fixture");
    let checks = validation["checks"].as_array().expect("valid test fixture");
    assert_eq!(checks.len(), 2);
    assert_eq!(
        checks
            .iter()
            .find(|row| row["source"] == "Synthetic Boss")
            .expect("valid test fixture")["status"],
        "censored"
    );
    assert_eq!(
        checks
            .iter()
            .find(|row| row["source"] == "Second Boss")
            .expect("valid test fixture")["status"],
        "matched"
    );
    assert!(!validation["model"].to_string().contains("zeta/1"));
    let kdl = fs::read_to_string(&timeline).expect("valid test fixture");
    assert_eq!(kdl.matches("branch ").count(), 2);
    assert_eq!(kdl.matches("open=0.0 close=4.0").count(), 2, "{kdl}");
    let reversed = dir.path().join("reversed.kdl");
    generate(
        &paths.into_iter().rev().collect::<Vec<_>>(),
        &reversed,
        mode,
    );
    assert_eq!(
        kdl,
        fs::read_to_string(reversed).expect("valid test fixture")
    );
    fs::remove_file(timeline.with_extension("evidence.json")).expect("valid test fixture");
    // No join or second option event is observed: the KDL must predict them without its evidence.
    for (second_only, second_start) in [(false, 11111), (true, 11111), (false, 3511)] {
        let recording = dir.path().join("recording.json");
        let mut observations = Vec::new();
        if !second_only {
            observations.extend([
                json!({"at_ms":1080,"type":"Ability","fields":{"id":"1","source":"Synthetic Boss"}}),
                json!({"at_ms":3080,"type":"Ability","fields":{"id":"2","source":"Synthetic Boss"}}),
            ]);
        }
        observations.extend([
            json!({"at_ms":second_start,"type":"Ability","fields":{"id":"1","source":"Second Boss"}}),
            json!({"at_ms":second_start+2000,"type":"Ability","fields":{"id":"3","source":"Second Boss"}}),
        ]);
        fs::write(
            &recording,
            serde_json::to_vec(&json!({"version":1,"end_ms":19000,"observations":observations}))
                .expect("valid test fixture"),
        )
        .expect("valid test fixture");
        let output = cargo_bin_cmd!("btimeline")
            .arg("compare")
            .arg(&timeline)
            .arg(&recording)
            .arg("--json")
            .assert()
            .success();
        let result: Value =
            serde_json::from_slice(&output.get_output().stdout).expect("valid test fixture");
        let rows = result["recordings"][recording.to_str().expect("valid test fixture")]
            .as_array()
            .expect("valid test fixture");
        assert!(
            !rows
                .iter()
                .any(|row| ["ambiguous", "violation", "unresolved"]
                    .contains(&row["action"].as_str().expect("valid test fixture"))),
            "{rows:?}"
        );
        let events: Vec<_> = rows.iter().filter(|row| row["action"] == "event").collect();
        let expected = if second_only {
            vec![1, 3, 2, 4]
        } else if second_start < 5100 {
            vec![1, 2, 1, 3, 2, 4]
        } else {
            vec![1, 2, 3, 4, 1, 3, 2, 4]
        };
        assert_eq!(events.len(), expected.len(), "{rows:?}");
        for (row, id) in events.iter().zip(expected) {
            assert!(
                row["detail"]
                    .as_str()
                    .expect("valid test fixture")
                    .contains(&format!("Skill {id}")),
                "{row}"
            );
        }
    }
}

#[test]
fn a_holdout_only_boss_is_unresolved_and_never_added_to_the_training_model() {
    let dir = tempfile::tempdir().expect("valid test fixture");
    let paths = [
        fixture(dir.path(), "alpha", false, [1080, 21111], false),
        fixture(dir.path(), "beta", true, [1111, 41080], false),
        fixture(dir.path(), "zeta", false, [3080, 31111], false),
    ];
    let path = &paths[2];
    let mut data: Value = serde_json::from_slice(&fs::read(path).expect("valid test fixture"))
        .expect("valid test fixture");
    data["report"]["masterData"]["actors"]
        .as_array_mut()
        .expect("valid test fixture")
        .push(json!({"id":12,"name":"Third Boss","gameID":99003,"type":"NPC","subType":"Boss"}));
    data["report"]["fights"][0]["enemyNPCs"]
        .as_array_mut()
        .expect("valid test fixture")
        .push(json!({"id":12,"gameID":99003}));
    data["events"]
        .as_array_mut()
        .expect("valid test fixture")
        .push(json!({"timestamp":52000,"type":"cast","sourceID":12,"abilityGameID":1,"fight":1}));
    data["collection"]["eventCount"] = data["events"]
        .as_array()
        .expect("valid test fixture")
        .len()
        .into();
    fs::write(path, serde_json::to_vec(&data).expect("valid test fixture"))
        .expect("valid test fixture");
    let evidence = generate(&paths, &dir.path().join("timeline.kdl"), "dungeon");
    let validation = evidence["alignment_diagnostics"]
        .as_array()
        .expect("valid test fixture")
        .iter()
        .find(|row| row["method"] == "automatic-draft-validation-v1")
        .expect("valid test fixture");
    assert_eq!(
        validation["checks"]
            .as_array()
            .expect("valid test fixture")
            .iter()
            .find(|row| row["source"] == "Third Boss")
            .expect("valid test fixture")["status"],
        "unresolved"
    );
    assert!(!validation["model"].to_string().contains("Third Boss"));
    assert!(!validation["model"].to_string().contains("zeta/1"));
}

#[test]
fn a_boss_opening_choice_requires_a_shared_entry_signal() {
    let dir = tempfile::tempdir().expect("valid test fixture");
    let paths = [
        fixture(dir.path(), "alpha", false, [1080, 21111], false),
        fixture(dir.path(), "beta", true, [1111, 41080], false),
    ];
    for path in &paths {
        let mut data: Value = serde_json::from_slice(&fs::read(path).expect("valid test fixture"))
            .expect("valid test fixture");
        data["events"]
            .as_array_mut()
            .expect("valid test fixture")
            .retain(|event| event["abilityGameID"] != 1);
        data["collection"]["eventCount"] = data["events"]
            .as_array()
            .expect("valid test fixture")
            .len()
            .into();
        fs::write(path, serde_json::to_vec(&data).expect("valid test fixture"))
            .expect("valid test fixture");
    }
    let output = cargo_bin_cmd!("btimeline")
        .arg("generate")
        .args(paths)
        .args(["--mode", "dungeon", "-o"])
        .arg(dir.path().join("timeline.kdl"))
        .assert()
        .failure();
    assert!(
        String::from_utf8_lossy(&output.get_output().stderr)
            .contains("Representative routes differ")
    );
}
