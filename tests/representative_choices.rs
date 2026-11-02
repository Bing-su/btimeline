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
    let validation = evidence["alignment_diagnostics"]
        .as_array()
        .expect("diagnostics")
        .iter()
        .find(|row| row["method"] == "automatic-draft-validation-v1")
        .expect("draft validation");
    assert_eq!(validation["status"], "insufficient-reports");
    assert_eq!(validation["reports"], 2);
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
fn automatic_validation_withholds_the_entire_report_before_selecting_a_representative() {
    let dir = tempfile::tempdir().expect("temporary directory");
    let mut paths = vec![
        fixture(dir.path(), "alpha", &[1, 2, 3, 4]),
        fixture(dir.path(), "beta", &[1, 3, 2, 4]),
        fixture(dir.path(), "zeta", &[1, 2, 3, 4]),
    ];
    let extra = dir.path().join("zeta-second.json");
    let mut data: Value =
        serde_json::from_slice(&fs::read(paths.last().expect("zeta fixture")).expect("fixture"))
            .expect("fixture JSON");
    data["report"]["fights"][0]["id"] = 2.into();
    data["collection"]["fightID"] = 2.into();
    for event in data["events"].as_array_mut().expect("events") {
        event["fight"] = 2.into();
    }
    data["events"][3]["timestamp"] = 18000.into();
    fs::write(&extra, serde_json::to_vec(&data).expect("encode fixture")).expect("save fixture");
    paths.push(extra);
    let evidence = generate(&paths, &dir.path().join("timeline.kdl"));
    assert_eq!(evidence["draft"]["routes"][0]["reference"], "zeta/2");
    let validation = evidence["alignment_diagnostics"]
        .as_array()
        .expect("diagnostics")
        .iter()
        .find(|row| row["method"] == "automatic-draft-validation-v1")
        .expect("draft validation");
    assert_eq!(validation["status"], "evaluated");
    assert_eq!(
        validation["split"]["training"],
        json!(["alpha/1", "beta/1"])
    );
    assert_eq!(validation["split"]["held_out"], json!(["zeta/1", "zeta/2"]));
    assert_eq!(validation["model"]["routes"][0]["reference"], "alpha/1");
    assert!(
        !serde_json::to_string(&validation["model"])
            .expect("model JSON")
            .contains("zeta/")
    );
    assert!(
        validation["checks"]
            .as_array()
            .expect("checks")
            .iter()
            .all(|check| check["status"] == "matched" && check["training"] == false)
    );
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
fn a_wipe_before_the_choice_preserves_shared_evidence_without_selecting_an_option() {
    let dir = tempfile::tempdir().expect("temporary directory");
    let mut paths = vec![
        fixture(dir.path(), "alpha", &[1, 2, 3, 4]),
        fixture(dir.path(), "beta", &[1, 3, 2, 4]),
    ];
    let baseline = dir.path().join("baseline.kdl");
    generate(&paths, &baseline);
    let short = fixture(dir.path(), "zeta", &[1]);
    let mut data: Value =
        serde_json::from_slice(&fs::read(&short).expect("fixture")).expect("fixture JSON");
    for pointer in [
        "/report/endTime",
        "/report/fights/0/endTime",
        "/collection/endTime",
    ] {
        *data.pointer_mut(pointer).expect("end time") = 3500.into();
    }
    fs::write(&short, serde_json::to_vec(&data).expect("encode fixture")).expect("save fixture");
    paths.push(short);
    let output = dir.path().join("with-prefix.kdl");
    let evidence = generate(&paths, &output);
    assert_eq!(
        fs::read(&baseline).expect("baseline"),
        fs::read(&output).expect("timeline")
    );
    let route = &evidence["draft"]["routes"][0];
    assert_eq!(route["peers_complete"], true);
    assert_eq!(route["nodes"][0]["observation_nodes"]["zeta/1"], 0);
    for option in route["choices"][0]["options"].as_array().expect("options") {
        assert!(
            !serde_json::to_string(option)
                .expect("option JSON")
                .contains("zeta/1")
        );
    }
    let check = evidence["draft"]["diagnostics"]
        .as_array()
        .expect("diagnostics")
        .iter()
        .find(|row| row["method"] == "representative-choice-prefix-v2" && row["trace"] == "zeta/1")
        .expect("prefix check");
    assert_eq!(check["status"], "censored");
    assert_eq!(check["selection"], "unobserved");
    assert_eq!(check["next_expected_ms"], 3000);
    let held_out = &evidence["alignment_diagnostics"]
        .as_array()
        .expect("validation diagnostics")
        .iter()
        .find(|row| row["method"] == "automatic-draft-validation-v1")
        .expect("validation")["checks"][0];
    assert_eq!(held_out["status"], "censored");
}

#[rstest::rstest]
#[case(vec![1,2], 2)]
#[case(vec![1,3], 3)]
fn a_wipe_after_selection_binds_only_the_observed_option_prefix(
    #[case] ids: Vec<i64>,
    #[case] selected: i64,
) {
    let dir = tempfile::tempdir().expect("temporary directory");
    let mut paths = vec![
        fixture(dir.path(), "alpha", &[1, 2, 3, 4]),
        fixture(dir.path(), "beta", &[1, 3, 2, 4]),
    ];
    let baseline = dir.path().join("baseline.kdl");
    generate(&paths, &baseline);
    let short = fixture(dir.path(), "zeta", &ids);
    let mut data: Value =
        serde_json::from_slice(&fs::read(&short).expect("fixture")).expect("fixture JSON");
    for pointer in [
        "/report/endTime",
        "/report/fights/0/endTime",
        "/collection/endTime",
    ] {
        *data.pointer_mut(pointer).expect("end time") = 5500.into();
    }
    fs::write(&short, serde_json::to_vec(&data).expect("encode fixture")).expect("save fixture");
    paths.push(short);
    let output = dir.path().join("with-prefix.kdl");
    let evidence = generate(&paths, &output);
    assert_eq!(
        fs::read(&baseline).expect("baseline"),
        fs::read(&output).expect("timeline")
    );
    let route = &evidence["draft"]["routes"][0];
    assert_eq!(route["peers_complete"], true);
    assert!(
        !route["nodes"][1]["observation_nodes"]
            .as_object()
            .expect("join bindings")
            .contains_key("zeta/1")
    );
    let options = route["choices"][0]["options"].as_array().expect("options");
    for option in options {
        if option["nodes"][0]["activity"]["identifier"]["Ability"] == selected {
            assert_eq!(option["nodes"][0]["observation_nodes"]["zeta/1"], 1);
            assert!(
                !option["nodes"][1]["observation_nodes"]
                    .as_object()
                    .expect("future bindings")
                    .contains_key("zeta/1")
            );
        } else {
            assert!(
                !serde_json::to_string(option)
                    .expect("option JSON")
                    .contains("zeta/1")
            );
        }
    }
    let validation = evidence["alignment_diagnostics"]
        .as_array()
        .expect("validation diagnostics")
        .iter()
        .find(|row| row["method"] == "automatic-draft-validation-v1")
        .expect("validation");
    assert_eq!(validation["checks"][0]["status"], "censored");
    assert_eq!(validation["checks"][0]["next_expected_ms"], 5000);
    assert!(
        !serde_json::to_string(&validation["model"])
            .expect("model JSON")
            .contains("zeta/1")
    );
}

#[test]
fn an_observed_join_can_bind_a_partial_common_suffix_after_the_selected_option() {
    let dir = tempfile::tempdir().expect("temporary directory");
    let mut paths = vec![
        fixture(dir.path(), "alpha", &[1, 2, 3, 4, 5]),
        fixture(dir.path(), "beta", &[1, 3, 2, 4, 5]),
    ];
    let short = fixture(dir.path(), "zeta", &[1, 3, 2, 4]);
    let mut data: Value =
        serde_json::from_slice(&fs::read(&short).expect("fixture")).expect("fixture JSON");
    for pointer in [
        "/report/endTime",
        "/report/fights/0/endTime",
        "/collection/endTime",
    ] {
        *data.pointer_mut(pointer).expect("end time") = 9500.into();
    }
    fs::write(&short, serde_json::to_vec(&data).expect("encode fixture")).expect("save fixture");
    paths.push(short);
    let evidence = generate(&paths, &dir.path().join("timeline.kdl"));
    let route = &evidence["draft"]["routes"][0];
    assert_eq!(route["nodes"][1]["activity"]["identifier"]["Ability"], 4);
    assert_eq!(route["nodes"][1]["observation_nodes"]["zeta/1"], 3);
    assert!(
        !route["nodes"][2]["observation_nodes"]
            .as_object()
            .expect("future bindings")
            .contains_key("zeta/1")
    );
    assert_eq!(route["peers_complete"], true);
}

#[rstest::rstest]
#[case(false)]
#[case(true)]
fn only_matching_options_set_the_prefix_deadline(#[case] faster_same_option: bool) {
    let dir = tempfile::tempdir().expect("temporary directory");
    let mut paths = vec![
        fixture(dir.path(), "alpha", &[1, 2, 3, 4]),
        fixture(dir.path(), "beta", &[1, 3, 2, 4]),
    ];
    for (path, times) in paths
        .iter()
        .zip([[2000, 4000, 10000, 14000], [2000, 4000, 5000, 14000]])
    {
        let mut data: Value =
            serde_json::from_slice(&fs::read(path).expect("fixture")).expect("fixture JSON");
        for (event, at) in data["events"]
            .as_array_mut()
            .expect("events")
            .iter_mut()
            .zip(times)
        {
            event["timestamp"] = at.into();
        }
        fs::write(path, serde_json::to_vec(&data).expect("encode fixture")).expect("save fixture");
    }
    if faster_same_option {
        let fast = fixture(dir.path(), "gamma", &[1, 2, 3, 4]);
        let mut data: Value =
            serde_json::from_slice(&fs::read(&fast).expect("fixture")).expect("fixture JSON");
        data["events"][3]["timestamp"] = 14000.into();
        fs::write(&fast, serde_json::to_vec(&data).expect("encode fixture")).expect("save fixture");
        paths.push(fast);
    }
    let short = fixture(dir.path(), "zeta", &[1, 2]);
    let mut data: Value =
        serde_json::from_slice(&fs::read(&short).expect("fixture")).expect("fixture JSON");
    for pointer in [
        "/report/endTime",
        "/report/fights/0/endTime",
        "/collection/endTime",
    ] {
        *data.pointer_mut(pointer).expect("end time") = 8000.into();
    }
    fs::write(&short, serde_json::to_vec(&data).expect("encode fixture")).expect("save fixture");
    paths.push(short);
    let output = dir.path().join("timeline.kdl");
    if faster_same_option {
        cargo_bin_cmd!("btimeline")
            .arg("generate")
            .args(paths)
            .args(["--mode", "raid", "-o"])
            .arg(&output)
            .assert()
            .failure();
        assert!(!output.exists());
    } else {
        let evidence = generate(&paths, &output);
        let check = evidence["draft"]["diagnostics"]
            .as_array()
            .expect("diagnostics")
            .iter()
            .find(|row| row["method"] == "representative-choice-prefix-v2")
            .expect("prefix check");
        assert_eq!(check["status"], "censored");
        assert_eq!(check["next_expected_ms"], 9000);
    }
}

#[test]
fn delayed_prefix_samples_do_not_invert_the_learned_option_schedule() {
    let dir = tempfile::tempdir().expect("temporary directory");
    let mut paths = vec![
        fixture(dir.path(), "alpha", &[1, 2, 3, 4, 5]),
        fixture(dir.path(), "beta", &[1, 4, 3, 2, 5]),
    ];
    let baseline = dir.path().join("baseline.kdl");
    generate(&paths, &baseline);
    for code in ["short-a", "short-b"] {
        let short = fixture(dir.path(), code, &[1, 2, 3]);
        let mut data: Value =
            serde_json::from_slice(&fs::read(&short).expect("fixture")).expect("fixture JSON");
        data["events"][2]["timestamp"] = 9500.into();
        for pointer in [
            "/report/endTime",
            "/report/fights/0/endTime",
            "/collection/endTime",
        ] {
            *data.pointer_mut(pointer).expect("end time") = 10000.into();
        }
        fs::write(&short, serde_json::to_vec(&data).expect("encode fixture"))
            .expect("save fixture");
        paths.push(short);
    }
    let output = dir.path().join("with-prefix.kdl");
    let evidence = generate(&paths, &output);
    assert_eq!(
        fs::read(&baseline).expect("baseline"),
        fs::read(&output).expect("timeline")
    );
    let event = evidence["events"]
        .as_array()
        .expect("events")
        .iter()
        .find(|event| {
            event["activity"]["identifier"]["Ability"] == 3 && event["option"] == "skill-2-2"
        })
        .expect("left middle event");
    assert_eq!(
        event["samples"].as_array().expect("timing samples").len(),
        1
    );
    assert_eq!(
        event["alignment"]["supported_observation_nodes"]
            .as_object()
            .expect("supported nodes")
            .len(),
        3
    );
    let records = replay(&output, Some((2, 3000)), 12000, None);
    let events: Vec<_> = records
        .iter()
        .filter(|row| row["action"] == "event")
        .collect();
    for (event, (id, at)) in
        events
            .iter()
            .zip([(1, 1000), (2, 3000), (3, 5000), (4, 7000), (5, 9000)])
    {
        assert!(
            event["detail"]
                .as_str()
                .expect("event detail")
                .contains(&format!("Skill {id}"))
        );
        assert_eq!(event["at_ms"], at);
    }
    assert_eq!(events.len(), 5);
}

#[rstest::rstest]
#[case(vec![1,2], 6000)]
#[case(vec![1,2], 6500)]
#[case(vec![1,2,4], 7500)]
fn overdue_or_non_prefix_option_observations_are_not_silently_censored(
    #[case] ids: Vec<i64>,
    #[case] end: i64,
) {
    let dir = tempfile::tempdir().expect("temporary directory");
    let mut paths = vec![
        fixture(dir.path(), "alpha", &[1, 2, 3, 4]),
        fixture(dir.path(), "beta", &[1, 3, 2, 4]),
    ];
    let short = fixture(dir.path(), "zeta", &ids);
    let mut data: Value =
        serde_json::from_slice(&fs::read(&short).expect("fixture")).expect("fixture JSON");
    for pointer in [
        "/report/endTime",
        "/report/fights/0/endTime",
        "/collection/endTime",
    ] {
        *data.pointer_mut(pointer).expect("end time") = end.into();
    }
    fs::write(&short, serde_json::to_vec(&data).expect("encode fixture")).expect("save fixture");
    paths.push(short);
    let output = dir.path().join("timeline.kdl");
    cargo_bin_cmd!("btimeline")
        .arg("generate")
        .args(paths)
        .args(["--mode", "raid", "-o"])
        .arg(&output)
        .assert()
        .failure();
    assert!(!output.exists());
    assert!(!output.with_extension("evidence.json").exists());
}

#[test]
fn close_competing_signals_do_not_establish_a_choice_in_a_short_prefix() {
    let dir = tempfile::tempdir().expect("temporary directory");
    let mut paths = vec![
        fixture(dir.path(), "alpha", &[1, 2, 3, 4]),
        fixture(dir.path(), "beta", &[1, 3, 2, 4]),
    ];
    let short = fixture(dir.path(), "zeta", &[1, 2, 3]);
    let mut data: Value =
        serde_json::from_slice(&fs::read(&short).expect("fixture")).expect("fixture JSON");
    data["events"][2]["timestamp"] = 4080.into();
    for pointer in [
        "/report/endTime",
        "/report/fights/0/endTime",
        "/collection/endTime",
    ] {
        *data.pointer_mut(pointer).expect("end time") = 4500.into();
    }
    fs::write(&short, serde_json::to_vec(&data).expect("encode fixture")).expect("save fixture");
    paths.push(short);
    let output = dir.path().join("timeline.kdl");
    cargo_bin_cmd!("btimeline")
        .arg("generate")
        .args(paths)
        .args(["--mode", "raid", "-o"])
        .arg(&output)
        .assert()
        .failure();
    assert!(!output.exists());
    assert!(!output.with_extension("evidence.json").exists());
}

#[test]
fn a_short_shared_prefix_can_relax_order_without_losing_its_later_relations() {
    let dir = tempfile::tempdir().expect("temporary directory");
    let mut paths = vec![
        fixture(dir.path(), "alpha", &[1, 2, 3, 4, 5]),
        fixture(dir.path(), "beta", &[1, 2, 4, 3, 5]),
    ];
    let short = fixture(dir.path(), "zeta", &[1, 2]);
    let mut data: Value =
        serde_json::from_slice(&fs::read(&short).expect("fixture")).expect("fixture JSON");
    data["events"][1]["timestamp"] = 2080.into();
    for pointer in [
        "/report/endTime",
        "/report/fights/0/endTime",
        "/collection/endTime",
    ] {
        *data.pointer_mut(pointer).expect("end time") = 3500.into();
    }
    fs::write(&short, serde_json::to_vec(&data).expect("encode fixture")).expect("save fixture");
    paths.push(short);
    let evidence = generate(&paths, &dir.path().join("timeline.kdl"));
    let route = &evidence["draft"]["routes"][0];
    assert_eq!(route["before"], json!([[0, 2], [1, 2]]));
    assert_eq!(route["nodes"][1]["observation_nodes"]["zeta/1"], 1);
    assert!(
        !route["nodes"][2]["observation_nodes"]
            .as_object()
            .expect("join bindings")
            .contains_key("zeta/1")
    );
}

#[rstest::rstest]
#[case(4000)]
#[case(4500)]
fn a_missing_choice_signal_is_not_excused_by_the_later_shared_join(#[case] end: i64) {
    let dir = tempfile::tempdir().expect("temporary directory");
    let mut paths = vec![
        fixture(dir.path(), "alpha", &[1, 2, 3, 4]),
        fixture(dir.path(), "beta", &[1, 3, 2, 4]),
    ];
    let short = fixture(dir.path(), "zeta", &[1]);
    let mut data: Value =
        serde_json::from_slice(&fs::read(&short).expect("fixture")).expect("fixture JSON");
    for pointer in [
        "/report/endTime",
        "/report/fights/0/endTime",
        "/collection/endTime",
    ] {
        *data.pointer_mut(pointer).expect("end time") = end.into();
    }
    fs::write(&short, serde_json::to_vec(&data).expect("encode fixture")).expect("save fixture");
    paths.push(short);
    let output = dir.path().join("timeline.kdl");
    cargo_bin_cmd!("btimeline")
        .arg("generate")
        .args(paths)
        .args(["--mode", "raid", "-o"])
        .arg(&output)
        .assert()
        .failure();
    assert!(!output.exists());
    assert!(!output.with_extension("evidence.json").exists());
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
