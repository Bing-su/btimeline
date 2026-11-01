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
fn wipes_are_checked_without_training_or_suppressing_internal_gaps(
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
        "wipes must not affect learned events"
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
fn explicit_holdouts_never_train_and_clears_cannot_discard_a_missing_suffix(
    #[case] kill: bool,
    #[case] sequence: Vec<i64>,
    #[case] method: &str,
    #[case] tail_status: &str,
    #[case] end: Option<i64>,
) {
    let dir = tempfile::tempdir().unwrap();
    let training = vec![
        input(dir.path(), 0, &[0, 1, 1, 2], true),
        input(dir.path(), 1, &[0, 1, 1, 2], true),
    ];
    let held_out = [
        input(dir.path(), 2, &sequence, kill),
        input(dir.path(), 3, &sequence, kill),
    ];
    if let Some(end) = end {
        for path in &held_out {
            let mut data: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
            data["report"]["fights"][0]["endTime"] = end.into();
            data["collection"]["endTime"] = end.into();
            fs::write(path, serde_json::to_vec(&data).unwrap()).unwrap();
        }
    }
    // Conflicting holdout display names must not change or reject the training-derived document.
    let mut holdout: Value = serde_json::from_slice(&fs::read(&held_out[0]).unwrap()).unwrap();
    holdout["report"]["masterData"]["abilities"][2]["name"] = "Holdout spelling".into();
    fs::write(&held_out[0], serde_json::to_vec(&holdout).unwrap()).unwrap();
    let baseline = dir.path().join("baseline.kdl");
    generate(&training, "raid", &baseline);
    let output = dir.path().join("checked.kdl");
    cargo_bin_cmd!("btimeline")
        .arg("generate")
        .args(&training)
        .args(["--mode", "raid", "-o"])
        .arg(&output)
        .arg("--holdout")
        .arg(&held_out[0])
        .arg("--holdout")
        .arg(&held_out[1])
        .assert()
        .success();
    assert_eq!(fs::read(baseline).unwrap(), fs::read(&output).unwrap());
    let report: Value =
        serde_json::from_slice(&fs::read(output.with_extension("evidence.json")).unwrap()).unwrap();
    assert_eq!(report["analysis"]["inputs"].as_array().unwrap().len(), 2);
    assert_eq!(
        report["holdout_analysis"]["inputs"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert!(
        report["events"]
            .as_array()
            .unwrap()
            .iter()
            .all(|event| event["samples"].as_array().unwrap().len() == 2)
    );
    let checks: Vec<_> = report["alignment_diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["held_out"] == true)
        .collect();
    assert_eq!(checks.len(), 2);
    for check in checks {
        assert_eq!(check["training"], false);
        assert_eq!(check["method"], method);
        let events = check["scopes"][0]["result"]["events"].as_array().unwrap();
        let tail = events
            .iter()
            .find(|event| event["event"]["activity"]["identifier"]["Ability"] == 46378)
            .unwrap();
        assert_eq!(tail["status"], tail_status);
        if kill && end.is_none() {
            assert!(events.iter().all(|event| event["status"] != "unobserved"));
        }
    }
}

#[rstest::rstest]
#[case("same-file")]
#[case("copied-file")]
#[case("same-trace-revised")]
#[case("encounter")]
#[case("difficulty")]
#[case("incomplete")]
#[case("duplicate-holdout")]
fn invalid_or_leaking_holdouts_leave_no_outputs(#[case] problem: &str) {
    let dir = tempfile::tempdir().unwrap();
    let training = vec![
        input(dir.path(), 0, &[0, 1, 1, 2], true),
        input(dir.path(), 1, &[0, 1, 1, 2], true),
    ];
    let mut held_out = input(dir.path(), 2, &[0, 1, 1, 2], true);
    let mut data: Value = serde_json::from_slice(&fs::read(&held_out).unwrap()).unwrap();
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
    fs::write(&held_out, serde_json::to_vec(&data).unwrap()).unwrap();
    if problem == "same-file" {
        held_out = training[0].clone();
    }
    if problem == "copied-file" {
        fs::copy(&training[0], &held_out).unwrap();
    }
    let output = dir.path().join("timeline.kdl");
    let mut command = cargo_bin_cmd!("btimeline");
    command
        .arg("generate")
        .args(&training)
        .args(["--mode", "raid", "-o"])
        .arg(&output)
        .arg("--holdout")
        .arg(&held_out);
    if problem == "duplicate-holdout" {
        command.arg("--holdout").arg(&held_out);
    }
    let result = command.assert().failure();
    assert!(result.get_output().stdout.is_empty());
    assert!(!output.exists());
    assert!(!output.with_extension("evidence.json").exists());
}

#[test]
fn one_training_sample_before_kill_prevents_future_censoring() {
    let dir = tempfile::tempdir().unwrap();
    let training = vec![
        input(dir.path(), 0, &[0, 2], true),
        input(dir.path(), 1, &[0, 2], true),
    ];
    let mut later: Value = serde_json::from_slice(&fs::read(&training[1]).unwrap()).unwrap();
    later["events"][1]["timestamp"] = 8000.into();
    fs::write(&training[1], serde_json::to_vec(&later).unwrap()).unwrap();
    let held_out = input(dir.path(), 2, &[0], true);
    let mut clear: Value = serde_json::from_slice(&fs::read(&held_out).unwrap()).unwrap();
    clear["report"]["fights"][0]["endTime"] = 6000.into();
    clear["collection"]["endTime"] = 6000.into();
    fs::write(&held_out, serde_json::to_vec(&clear).unwrap()).unwrap();
    let output = dir.path().join("timeline.kdl");
    cargo_bin_cmd!("btimeline")
        .arg("generate")
        .args(&training)
        .args(["--mode", "raid", "-o"])
        .arg(&output)
        .arg("--holdout")
        .arg(&held_out)
        .assert()
        .success();
    let report: Value =
        serde_json::from_slice(&fs::read(output.with_extension("evidence.json")).unwrap()).unwrap();
    let check = report["alignment_diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["held_out"] == true)
        .unwrap();
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
