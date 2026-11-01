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
