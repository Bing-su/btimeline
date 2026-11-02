use std::fs;
use std::path::{Path, PathBuf};

use assert_cmd::cargo::cargo_bin_cmd;
use serde_json::{Value, json};

fn fixture(directory: &Path, code: &str, ids: &[i64]) -> PathBuf {
    let mut data: Value =
        serde_json::from_str(include_str!("fixtures/pull.json")).expect("fixture");
    for (pointer, value) in [
        ("/report/code", code.into()), ("/collection/reportCode", code.into()),
        ("/report/fights/0/kill", false.into()),
        ("/report/masterData/abilities", (1..=8).map(|id| json!({"gameID":id,"name":format!("Skill {id}"),"type":"1"})).collect()),
        ("/events", ids.iter().enumerate().map(|(i,id)|json!({"timestamp":2000+i*2000,"type":"cast","sourceID":10,"abilityGameID":id,"fight":1})).collect()),
        ("/collection/eventCount", ids.len().into()),
    ] { *data.pointer_mut(pointer).expect("fixture field") = value; }
    let path = directory.join(format!("{code}.json"));
    fs::write(&path, serde_json::to_vec(&data).expect("encode fixture")).expect("write fixture");
    path
}

fn generate(paths: &[PathBuf], output: &Path) -> Value {
    cargo_bin_cmd!("btimeline")
        .arg("generate")
        .args(paths)
        .args(["--mode", "raid", "-o"])
        .arg(output)
        .assert()
        .success();
    serde_json::from_slice(&fs::read(output.with_extension("evidence.json")).expect("evidence"))
        .expect("evidence JSON")
}

fn replay(
    timeline: &Path,
    selected: Option<(i64, i64)>,
    end: i64,
    earlier: Option<i64>,
) -> Vec<Value> {
    let recording = timeline.with_extension("recording.json");
    let mut observations = vec![json!({"at_ms":0,"type":"InCombat","fields":{"inGameCombat":"1"}})];
    if let Some(id) = earlier {
        observations.push(json!({"at_ms":2000,"type":"Ability","fields":{"id":format!("{id:X}"),"source":"Synthetic Boss"}}));
    }
    if let Some((id, at_ms)) = selected {
        observations.push(json!({"at_ms":at_ms,"type":"Ability","fields":{"id":format!("{id:X}"),"source":"Synthetic Boss"}}));
    }
    fs::write(
        &recording,
        serde_json::to_vec(&json!({"version":1,"end_ms":end,"observations":observations}))
            .expect("encode recording"),
    )
    .expect("recording");
    let output = cargo_bin_cmd!("btimeline")
        .arg("compare")
        .arg(timeline)
        .arg(&recording)
        .arg("--json")
        .assert()
        .success();
    let report: Value = serde_json::from_slice(&output.get_output().stdout).expect("replay JSON");
    report
        .get("recordings")
        .and_then(|recordings| recordings.get(recording.to_str().expect("recording path")))
        .expect("recording results")
        .as_array()
        .expect("records")
        .clone()
}

#[rstest::rstest]
#[case(Some(2), vec![1,2,3,4])]
#[case(Some(3), vec![1,3,2,4])]
#[case(None, vec![1,4])]
fn wipe_order_choices_predict_the_selected_path_without_evidence(
    #[case] selected: Option<i64>,
    #[case] expected: Vec<i64>,
) {
    let dir = tempfile::tempdir().expect("temporary directory");
    let paths = vec![
        fixture(dir.path(), "alpha", &[1, 2, 3, 4]),
        fixture(dir.path(), "beta", &[1, 3, 2, 4]),
    ];
    let timeline = dir.path().join("timeline.kdl");
    let evidence = generate(&paths, &timeline);
    let route = &evidence["draft"]["routes"][0];
    assert_eq!(route["nodes"].as_array().expect("common nodes").len(), 2);
    assert_eq!(route["choices"][0]["after"], 0);
    assert_eq!(route["choices"][0]["join"], 1);
    assert_eq!(
        route["choices"][0]["options"]
            .as_array()
            .expect("options")
            .len(),
        2
    );
    assert_eq!(evidence["excluded"], json!([]));
    let reverse = dir.path().join("reverse.kdl");
    let reversed = generate(&paths.into_iter().rev().collect::<Vec<_>>(), &reverse);
    assert_eq!(evidence["draft"], reversed["draft"]);
    assert_eq!(
        fs::read(&timeline).expect("timeline"),
        fs::read(reverse).expect("reverse timeline")
    );
    fs::remove_file(timeline.with_extension("evidence.json")).expect("remove evidence");
    let records = replay(&timeline, selected.map(|id| (id, 3000)), 10000, None);
    let events: Vec<_> = records
        .iter()
        .filter(|row| row["action"] == "event")
        .collect();
    assert_eq!(events.len(), expected.len());
    for (event, id) in events.iter().zip(expected) {
        assert!(
            event["detail"]
                .as_str()
                .expect("event detail")
                .contains(&format!("Skill {id}"))
        );
    }
    assert!(
        !records
            .iter()
            .any(|row| row["action"] == "ambiguous" || row["action"] == "violation")
    );
    assert_eq!(
        records.iter().any(|row| row["action"] == "unresolved"),
        selected.is_none()
    );
}

#[test]
fn a_choice_can_begin_before_the_first_shared_cast() {
    let dir = tempfile::tempdir().expect("temporary directory");
    let paths = vec![
        fixture(dir.path(), "alpha", &[2, 3, 4]),
        fixture(dir.path(), "beta", &[3, 2, 4]),
    ];
    let timeline = dir.path().join("timeline.kdl");
    let evidence = generate(&paths, &timeline);
    assert_eq!(
        evidence["draft"]["routes"][0]["choices"][0]["after"],
        Value::Null
    );
    let records = replay(&timeline, Some((3, 1000)), 7000, None);
    let events: Vec<_> = records
        .iter()
        .filter(|row| row["action"] == "event")
        .collect();
    assert_eq!(events.len(), 3);
    for (event, (id, at)) in events.iter().zip([(3, 1000), (2, 3000), (4, 5000)]) {
        assert!(
            event["detail"]
                .as_str()
                .expect("event detail")
                .contains(&format!("Skill {id}"))
        );
        assert_eq!(event["at_ms"], at);
    }
}

#[test]
fn correlated_differences_stay_inside_one_observed_option() {
    let dir = tempfile::tempdir().expect("temporary directory");
    let paths = vec![
        fixture(dir.path(), "alpha", &[1, 2, 3, 4, 5, 6, 7]),
        fixture(dir.path(), "beta", &[1, 3, 2, 4, 6, 5, 7]),
    ];
    let timeline = dir.path().join("timeline.kdl");
    let evidence = generate(&paths, &timeline);
    let choices = evidence["draft"]["routes"][0]["choices"]
        .as_array()
        .expect("choices");
    assert_eq!(choices.len(), 1);
    let paths: Vec<Vec<i64>> = choices[0]["options"]
        .as_array()
        .expect("options")
        .iter()
        .map(|option| {
            option["nodes"]
                .as_array()
                .expect("nodes")
                .iter()
                .map(|node| {
                    node["activity"]["identifier"]["Ability"]
                        .as_i64()
                        .expect("ability")
                })
                .collect()
        })
        .collect();
    assert_eq!(paths, [vec![2, 3, 4, 5, 6], vec![3, 2, 4, 6, 5]]);
    for selected in [2, 3] {
        let records = replay(&timeline, Some((selected, 3000)), 16000, None);
        assert_eq!(
            records
                .iter()
                .filter(|row| row["action"] == "event")
                .count(),
            7
        );
        assert!(
            !records
                .iter()
                .any(|row| row["action"] == "unresolved" || row["action"] == "ambiguous")
        );
    }
}

#[test]
fn an_earlier_unclassified_copy_of_the_choice_signal_cannot_select_the_option() {
    let dir = tempfile::tempdir().expect("temporary directory");
    let paths = vec![
        fixture(dir.path(), "alpha", &[1, 2, 3, 4]),
        fixture(dir.path(), "beta", &[1, 3, 2, 4]),
    ];
    for (path, id) in paths.iter().zip([2, 3]) {
        let mut data: Value =
            serde_json::from_slice(&fs::read(path).expect("fixture")).expect("fixture JSON");
        data["report"]["masterData"]["actors"].as_array_mut().expect("actors").push(
            json!({"id":11,"gameID":99002,"name":"Synthetic Boss","type":"NPC","subType":"NPC"}));
        data["events"].as_array_mut().expect("events").push(
            json!({"timestamp":3000,"type":"cast","sourceID":11,"abilityGameID":id,"fight":1}),
        );
        data["collection"]["eventCount"] = 5.into();
        fs::write(path, serde_json::to_vec(&data).expect("encode fixture")).expect("save fixture");
    }
    let timeline = dir.path().join("timeline.kdl");
    generate(&paths, &timeline);
    let records = replay(&timeline, Some((2, 3000)), 10000, Some(2));
    let branch_events: Vec<_> = records
        .iter()
        .filter(|row| {
            row["action"] == "event"
                && row["detail"]
                    .as_str()
                    .expect("event detail")
                    .contains("before-4-via-2")
        })
        .collect();
    assert_eq!(branch_events.len(), 2);
    assert_eq!(branch_events[0]["at_ms"], 3000);
    assert_eq!(branch_events[1]["at_ms"], 5000);
}

#[rstest::rstest]
#[case("different-first-choice", vec![1,2,4,3,5], vec![1,3,2,4,5])]
#[case("not-a-permutation", vec![1,2,3,4], vec![1,3,5,4])]
#[case("near-simultaneous", vec![1,2,3,4], vec![1,3,2,4])]
fn unsupported_or_uncertain_choices_do_not_produce_a_guessed_path(
    #[case] scenario: &str,
    #[case] left: Vec<i64>,
    #[case] right: Vec<i64>,
) {
    let dir = tempfile::tempdir().expect("temporary directory");
    let mut paths = vec![
        fixture(dir.path(), "alpha", &left),
        fixture(dir.path(), "beta", &right),
    ];
    if scenario == "different-first-choice" {
        paths.push(fixture(dir.path(), "gamma", &[1, 2, 3, 4, 5]));
    }
    if scenario == "near-simultaneous" {
        for path in &paths {
            let mut data: Value =
                serde_json::from_slice(&fs::read(path).expect("fixture")).expect("fixture JSON");
            data["events"][2]["timestamp"] = 4050.into();
            fs::write(path, serde_json::to_vec(&data).expect("encode fixture"))
                .expect("save fixture");
        }
    }
    let timeline = dir.path().join("timeline.kdl");
    cargo_bin_cmd!("btimeline")
        .arg("generate")
        .args(paths)
        .args(["--mode", "raid", "-o"])
        .arg(&timeline)
        .assert()
        .failure();
    assert!(!timeline.exists());
    assert!(!timeline.with_extension("evidence.json").exists());
}
