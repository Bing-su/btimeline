use std::fs;
use std::path::{Path, PathBuf};

use assert_cmd::cargo::cargo_bin_cmd;
use serde_json::{Value, json};

fn input(directory: &Path, index: usize, sequence: &[i64], kill: bool) -> PathBuf {
    let mut data: Value =
        serde_json::from_str(include_str!("fixtures/pull.json")).expect("fixture");
    let abilities = (0..6)
        .map(|id| json!({"gameID":46376+id,"name":format!("Spell{id}"),"type":"1"}))
        .collect::<Value>();
    let events=sequence.iter().enumerate().map(|(i,id)|json!({"timestamp":2000+i*2000,"type":"cast","sourceID":10,"abilityGameID":46376+id,"fight":1})).collect::<Value>();
    for (pointer, value) in [
        ("/report/code", format!("attempt{index}").into()),
        ("/collection/reportCode", format!("attempt{index}").into()),
        ("/report/fights/0/kill", kill.into()),
        ("/report/masterData/abilities", abilities),
        ("/events", events),
        ("/collection/eventCount", sequence.len().into()),
    ] {
        *data.pointer_mut(pointer).expect("fixture field") = value;
    }
    if !kill {
        let end = 500 + sequence.len() * 2000;
        *data
            .pointer_mut("/report/fights/0/endTime")
            .expect("fight end") = end.into();
        *data
            .pointer_mut("/collection/endTime")
            .expect("collection end") = end.into();
    }
    let path = directory.join(format!("pull{index}.json"));
    fs::write(&path, serde_json::to_vec(&data).expect("serialize fixture")).expect("write fixture");
    path
}

#[rstest::rstest]
#[case("raid",vec![0,1],"matched","unobserved")]
#[case("dungeon",vec![0,2,3,4,5],"mismatch","mismatch")]
#[case("alliance",vec![0,1,2],"ambiguous","ambiguous")]
fn wipes_do_not_change_bounded_learning_or_suppress_internal_gaps(
    #[case] mode: &str,
    #[case] sequence: Vec<i64>,
    #[case] first: &str,
    #[case] second: &str,
) {
    let dir = tempfile::tempdir().unwrap();
    let mut paths = vec![
        input(dir.path(), 0, &[0, 1, 1, 2, 3, 4, 5], true),
        input(dir.path(), 1, &[0, 1, 1, 2, 3, 4, 5], true),
    ];
    let baseline = dir.path().join("baseline.kdl");
    generate(&paths, mode, &baseline);
    paths.push(input(dir.path(), 2, &sequence, false));
    let output = dir.path().join("with-wipe.kdl");
    generate(&paths, mode, &output);
    assert_eq!(
        fs::read(&baseline).unwrap(),
        fs::read(&output).unwrap(),
        "wipes outside a supported tail must not affect bounded events"
    );
    let report: Value =
        serde_json::from_slice(&fs::read(output.with_extension("evidence.json")).unwrap()).unwrap();
    let attempt = report["alignment_diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["method"] == "censored-reference-prefix-v1")
        .unwrap();
    assert_eq!(attempt["training"], false);
    let scope = &attempt["scopes"][0]["result"];
    assert_eq!(scope["status"], "evaluated");
    let repeats: Vec<_> = scope["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["event"]["activity"]["identifier"]["Ability"] == 46377)
        .collect();
    assert_eq!(repeats.len(), 2);
    assert_eq!(repeats[0]["status"], first);
    assert_eq!(repeats[1]["status"], second);
    assert_eq!(report["analysis"]["inputs"][2]["kill"], false);
    if mode == "raid" {
        insta::assert_json_snapshot!("wipe_prefix", scope);
    }
}

fn generate(paths: &[PathBuf], mode: &str, output: &Path) {
    cargo_bin_cmd!("btimeline")
        .arg("generate")
        .args(paths)
        .args(["--mode", mode, "-o"])
        .arg(output)
        .assert()
        .success();
}

#[rstest::rstest]
#[case(true,vec![0,1],"complete-reference-alignment-v1","mismatch",None)]
#[case(true,vec![0,1,1,2],"complete-reference-alignment-v1","matched",None)]
#[case(false,vec![0,1],"censored-reference-prefix-v1","unobserved",None)]
#[case(true,vec![0,1],"complete-reference-alignment-v1","unobserved",Some(4500))]
fn automatic_validation_uses_a_separate_model_before_final_refit(
    #[case] kill: bool,
    #[case] sequence: Vec<i64>,
    #[case] method: &str,
    #[case] tail_status: &str,
    #[case] end: Option<i64>,
) {
    let dir = tempfile::tempdir().unwrap();
    let paths = vec![
        input(dir.path(), 0, &[0, 1, 1, 2], true),
        input(dir.path(), 1, &[0, 1, 1, 2], true),
        input(dir.path(), 2, &sequence, kill),
    ];
    if let Some(end) = end {
        let mut data: Value = serde_json::from_slice(&fs::read(&paths[2]).unwrap()).unwrap();
        data["report"]["fights"][0]["endTime"] = end.into();
        data["collection"]["endTime"] = end.into();
        fs::write(&paths[2], serde_json::to_vec(&data).unwrap()).unwrap();
    }
    let output = dir.path().join("timeline.kdl");
    generate(&paths, "raid", &output);
    let report: Value =
        serde_json::from_slice(&fs::read(output.with_extension("evidence.json")).unwrap()).unwrap();
    let diagnostics = report["alignment_diagnostics"].as_array().unwrap();
    let validation = diagnostics
        .iter()
        .find(|row| row["method"] == "automatic-validation-v1")
        .unwrap();
    let check = if kill {
        assert_eq!(validation["status"], "evaluated");
        assert_eq!(validation["split"]["held_out"], "attempt2/1");
        assert_eq!(
            validation["split"]["training"],
            json!(["attempt0/1", "attempt1/1"])
        );
        assert!(
            validation["model_events"]
                .as_array()
                .unwrap()
                .iter()
                .all(|event| event["samples"].as_object().unwrap().len() == 2)
        );
        &validation["checks"][0]
    } else {
        assert_eq!(validation["status"], "insufficient-completed-pulls");
        diagnostics
            .iter()
            .find(|row| row["method"] == method)
            .unwrap()
    };
    assert_eq!(check["training"], false);
    assert_eq!(check["method"], method);
    let events = check["scopes"][0]["result"]["events"].as_array().unwrap();
    let tail = events
        .iter()
        .find(|event| event["event"]["activity"]["identifier"]["Ability"] == 46378)
        .unwrap();
    assert_eq!(tail["status"], tail_status);
    let count = if kill { 3 } else { 2 };
    for event in report["events"].as_array().unwrap() {
        let future =
            end.is_some_and(|end| event["median_offset_ms"].as_i64().unwrap() > end - 1000);
        assert_eq!(
            event["samples"].as_array().unwrap().len(),
            if future { 2 } else { count }
        );
        if future {
            assert_eq!(event["alignment"]["method"], "censored-tail-v1");
        }
    }
    let reversed: Vec<_> = paths.iter().rev().cloned().collect();
    let reordered = dir.path().join("reordered.kdl");
    generate(&reversed, "raid", &reordered);
    let repeated: Value =
        serde_json::from_slice(&fs::read(reordered.with_extension("evidence.json")).unwrap())
            .unwrap();
    let again = repeated["alignment_diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["method"] == "automatic-validation-v1")
        .unwrap();
    assert_eq!(validation, again);
    assert_eq!(fs::read(output).unwrap(), fs::read(reordered).unwrap());
}

#[rstest::rstest]
#[case("same-file")]
#[case("copied-file")]
#[case("same-trace-revised")]
#[case("encounter")]
#[case("difficulty")]
#[case("incomplete")]
#[case("duplicate-input")]
fn invalid_inputs_leave_no_outputs(#[case] problem: &str) {
    let dir = tempfile::tempdir().unwrap();
    let mut paths = vec![
        input(dir.path(), 0, &[0, 1, 1, 2], true),
        input(dir.path(), 1, &[0, 1, 1, 2], true),
        input(dir.path(), 2, &[0, 1, 1, 2], true),
    ];
    let mut data: Value = serde_json::from_slice(&fs::read(&paths[2]).unwrap()).unwrap();
    match problem {
        "same-trace-revised" => {
            data["report"]["code"] = "attempt0".into();
            data["collection"]["reportCode"] = "attempt0".into();
            data["collection"]["collectedAtUnixMs"] = 5.into();
        }
        "encounter" => data["report"]["fights"][0]["encounterID"] = 99098.into(),
        "difficulty" => data["report"]["fights"][0]["difficulty"] = 998.into(),
        "incomplete" => data["collection"]["complete"] = false.into(),
        _ => (),
    }
    fs::write(&paths[2], serde_json::to_vec(&data).unwrap()).unwrap();
    if problem == "same-file" {
        paths[2] = paths[0].clone();
    }
    if problem == "copied-file" {
        fs::copy(&paths[0], &paths[2]).unwrap();
    }
    if problem == "duplicate-input" {
        paths.push(paths[2].clone());
    }
    let output = dir.path().join("timeline.kdl");
    let result = cargo_bin_cmd!("btimeline")
        .arg("generate")
        .args(&paths)
        .args(["--mode", "raid", "-o"])
        .arg(&output)
        .assert()
        .failure();
    assert!(result.get_output().stdout.is_empty());
    assert!(!output.exists());
    assert!(!output.with_extension("evidence.json").exists());
}

#[test]
fn one_training_sample_before_kill_prevents_future_censoring() {
    let dir = tempfile::tempdir().unwrap();
    let paths = vec![
        input(dir.path(), 0, &[0, 2], true),
        input(dir.path(), 1, &[0, 2], true),
        input(dir.path(), 2, &[0], true),
    ];
    let mut later: Value = serde_json::from_slice(&fs::read(&paths[1]).unwrap()).unwrap();
    later["events"][1]["timestamp"] = 8000.into();
    fs::write(&paths[1], serde_json::to_vec(&later).unwrap()).unwrap();
    let mut clear: Value = serde_json::from_slice(&fs::read(&paths[2]).unwrap()).unwrap();
    clear["report"]["fights"][0]["endTime"] = 6000.into();
    clear["collection"]["endTime"] = 6000.into();
    fs::write(&paths[2], serde_json::to_vec(&clear).unwrap()).unwrap();
    let output = dir.path().join("timeline.kdl");
    generate(&paths, "raid", &output);
    let report: Value =
        serde_json::from_slice(&fs::read(output.with_extension("evidence.json")).unwrap()).unwrap();
    let validation = report["alignment_diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["method"] == "automatic-validation-v1")
        .unwrap();
    let check = &validation["checks"][0];
    let tail = check["scopes"][0]["result"]["events"]
        .as_array()
        .unwrap()
        .iter()
        .find(|event| event["event"]["activity"]["identifier"]["Ability"] == 46378)
        .unwrap();
    assert_eq!(tail["status"], "mismatch");
}

#[test]
fn wipe_without_an_observed_anchor_remains_unresolved() {
    let dir = tempfile::tempdir().unwrap();
    let paths = vec![
        input(dir.path(), 0, &[0, 1, 1, 2], true),
        input(dir.path(), 1, &[0, 1, 1, 2], true),
        input(dir.path(), 2, &[1, 1], false),
    ];
    let output = dir.path().join("timeline.kdl");
    generate(&paths, "raid", &output);
    let report: Value =
        serde_json::from_slice(&fs::read(output.with_extension("evidence.json")).unwrap()).unwrap();
    let attempt = report["alignment_diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["method"] == "censored-reference-prefix-v1")
        .unwrap();
    assert_eq!(attempt["scopes"][0]["result"]["status"], "unresolved");
}

#[test]
fn validation_order_conflicts_are_localized_instead_of_blocking_the_final_fit() {
    let dir = tempfile::tempdir().unwrap();
    let paths = vec![
        input(dir.path(), 0, &[0, 1, 2, 3], true),
        input(dir.path(), 1, &[0, 2, 1, 3], true),
        input(dir.path(), 2, &[0, 3], true),
    ];
    let output = dir.path().join("timeline.kdl");
    generate(&paths, "raid", &output);
    let report: Value =
        serde_json::from_slice(&fs::read(output.with_extension("evidence.json")).unwrap()).unwrap();
    assert_eq!(report["generation_method"], "common-backbone-v1");
    assert_eq!(report["draft"]["routes"][0]["peers_complete"], false);
    let validation = report["alignment_diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["method"] == "automatic-validation-v1")
        .unwrap();
    assert_eq!(validation["status"], "evaluated");
    assert!(
        validation["model_diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["method"] == "local-order-branch-v1" && row["status"] == "generated")
    );
    assert_eq!(report["events"].as_array().unwrap().len(), 2);
}

#[test]
fn validation_scope_does_not_learn_boss_metadata_from_the_selected_log() {
    let dir = tempfile::tempdir().unwrap();
    let paths: Vec<_> = (0..3)
        .map(|index| input(dir.path(), index, &[0, 1, 1, 2], true))
        .collect();
    for path in paths.iter().take(2) {
        let mut data: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        data["report"]["masterData"]["actors"][0]["subType"] = "NPC".into();
        fs::write(path, serde_json::to_vec(&data).unwrap()).unwrap();
    }
    let mut selected: Value = serde_json::from_slice(&fs::read(&paths[2]).unwrap()).unwrap();
    selected["report"]["masterData"]["actors"][0]["gameID"] = 99002.into();
    selected["report"]["fights"][0]["enemyNPCs"][0]["gameID"] = 99002.into();
    fs::write(&paths[2], serde_json::to_vec(&selected).unwrap()).unwrap();
    let output = dir.path().join("timeline.kdl");
    generate(&paths, "dungeon", &output);
    let report: Value =
        serde_json::from_slice(&fs::read(output.with_extension("evidence.json")).unwrap()).unwrap();
    let validation = report["alignment_diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["method"] == "automatic-validation-v1")
        .unwrap();
    assert_eq!(validation["status"], "unresolved");
    assert_eq!(validation["reason"], "no common events in validation model");
    assert!(!report["events"].as_array().unwrap().is_empty());
}

#[test]
fn a_long_silent_tail_does_not_turn_due_events_into_censored_success() {
    let dir = tempfile::tempdir().unwrap();
    let paths = vec![
        input(dir.path(), 0, &[0, 1, 1, 2], true),
        input(dir.path(), 1, &[0, 1, 1, 2], true),
        input(dir.path(), 2, &[0, 1], false),
    ];
    let mut wipe: Value = serde_json::from_slice(&fs::read(&paths[2]).unwrap()).unwrap();
    wipe["report"]["fights"][0]["endTime"] = 19000.into();
    wipe["collection"]["endTime"] = 19000.into();
    fs::write(&paths[2], serde_json::to_vec(&wipe).unwrap()).unwrap();
    let output = dir.path().join("timeline.kdl");
    generate(&paths, "raid", &output);
    let report: Value =
        serde_json::from_slice(&fs::read(output.with_extension("evidence.json")).unwrap()).unwrap();
    let checks = &report["alignment_diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["method"] == "censored-reference-prefix-v1")
        .unwrap()["scopes"][0]["result"]["events"];
    let later = checks
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["event"]["reference_at_ms"] == 5000)
        .unwrap();
    assert_eq!(later["status"], "unresolved");
    assert_eq!(
        later["reason"],
        "censored alignment but training timing overlaps observed duration"
    );
}

#[test]
fn a_reordered_validation_anchor_does_not_hide_earlier_observations() {
    let dir = tempfile::tempdir().unwrap();
    let paths = vec![
        input(dir.path(), 0, &[0, 1, 2, 3], true),
        input(dir.path(), 1, &[0, 1, 2, 3], true),
        input(dir.path(), 2, &[2, 0, 1, 3], true),
    ];
    let output = dir.path().join("timeline.kdl");
    generate(&paths, "raid", &output);
    let report: Value =
        serde_json::from_slice(&fs::read(output.with_extension("evidence.json")).unwrap()).unwrap();
    let validation = report["alignment_diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["method"] == "automatic-draft-validation-v1")
        .unwrap();
    assert_eq!(validation["checks"][0]["status"], "unresolved");
    assert_eq!(validation["split"]["held_out"], json!(["attempt2/1"]));
    assert!(
        !serde_json::to_string(&validation["model"])
            .unwrap()
            .contains("attempt2/1")
    );
    assert_eq!(
        report["draft"]["routes"][0]["choices"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}
