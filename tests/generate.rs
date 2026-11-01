use std::fs;
use std::path::{Path, PathBuf};

use assert_cmd::cargo::cargo_bin_cmd;
use serde_json::{Value, json};

const INPUT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/pull.json");

#[rstest::rstest]
#[case("raid", "same", vec![1,2])]
#[case("dungeon", "swapped", vec![1,2])]
#[case("alliance", "jitter", vec![1,2])]
#[case("raid", "missing", vec![])]
#[case("raid", "duplicates", vec![3])]
fn uncertainty_groups_preserve_multiplicity_and_individual_samples(
    #[case] mode: &str,
    #[case] problem: &str,
    #[case] ordinals: Vec<usize>,
) {
    let dir = tempfile::tempdir().unwrap();
    let paths = inputs(dir.path());
    for (index, path) in paths.iter().enumerate() {
        let mut data: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        data["report"]["masterData"]["abilities"] = (0..4)
            .map(|id| json!({"gameID":46376+id,"name":format!("Spell{id}"),"type":"1"}))
            .collect();
        let records = uncertainty_records(problem, index);
        data["collection"]["eventCount"] = records.len().into();
        data["events"]=records.iter().map(|(at,id)|json!({"timestamp":at,"type":"cast","sourceID":10,"abilityGameID":46376+id,"fight":1})).collect();
        fs::write(path, serde_json::to_vec(&data).unwrap()).unwrap();
    }
    let output = dir.path().join("timeline.kdl");
    cargo_bin_cmd!("btimeline")
        .arg("generate")
        .args(&paths)
        .args(["--mode", mode, "-o"])
        .arg(&output)
        .assert()
        .success();
    let evidence: Value =
        serde_json::from_slice(&fs::read(output.with_extension("evidence.json")).unwrap()).unwrap();
    let events: Vec<_> = evidence["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|event| event["activity"]["identifier"]["Ability"] == 46377)
        .collect();
    assert_eq!(events.len(), ordinals.len());
    for (event, ordinal) in events.iter().zip(ordinals) {
        assert!(
            event["event"]
                .as_str()
                .unwrap()
                .ends_with(&format!("-{ordinal:02}"))
        );
        assert_eq!(event["alignment"]["batches"].as_object().unwrap().len(), 2);
    }
    if problem == "jitter" {
        assert_eq!(events[0]["samples"][0]["at_ms"], 3000);
        assert_eq!(events[0]["samples"][1]["at_ms"], 3050);
        assert_eq!(
            events[0]["alignment"]["groups"]["generation1/1"],
            json!([1, 2])
        );
    }
    if problem == "same" {
        insta::assert_snapshot!(
            "generated_uncertainty_groups",
            fs::read_to_string(&output).unwrap()
        );
        // The uncertainty groups are mining evidence; the external execution input stays plain KDL.
        fs::remove_file(output.with_extension("evidence.json")).unwrap();
        let recording = dir.path().join("recording.json");
        fs::write(
            &recording,
            serde_json::to_vec(&json!({"version":1,"end_ms":12000,"observations":[
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
        let report: Value = serde_json::from_slice(&replay.get_output().stdout).unwrap();
        let records = report["recordings"][recording.to_str().unwrap()]
            .as_array()
            .unwrap();
        assert_eq!(
            records
                .iter()
                .filter(|row| row["action"] == "event")
                .count(),
            6
        );
    }
}

fn uncertainty_records(problem: &str, index: usize) -> Vec<(i64, i64)> {
    match (problem, index) {
        ("duplicates", _) => vec![(2000, 0), (4000, 1), (4050, 1), (8000, 1), (12000, 2)],
        ("swapped", 1) => vec![
            (2000, 0),
            (4000, 3),
            (4000, 1),
            (8000, 3),
            (8000, 1),
            (12000, 2),
        ],
        ("jitter", 1) => vec![
            (2000, 0),
            (4000, 3),
            (4050, 1),
            (8000, 3),
            (8050, 1),
            (12000, 2),
        ],
        ("missing", 1) => vec![(2000, 0), (4000, 1), (8000, 1), (8000, 3), (12000, 2)],
        _ => vec![
            (2000, 0),
            (4000, 1),
            (4000, 3),
            (8000, 1),
            (8000, 3),
            (12000, 2),
        ],
    }
}

#[rstest::rstest]
#[case("equal", vec![0,1,1,2], vec![0,1,1,2], vec!["-01","-02"])]
#[case("ambiguous", vec![0,1,1,2], vec![0,1,2], vec![])]
#[case("context", vec![0,1,2,1,3], vec![0,2,1,3], vec!["-02"])]
#[case("nearby", vec![0,1,1,2], vec![0,1,1,2], vec![])]
#[case("phase", vec![0,1,1,2], vec![0,1,1,2], vec![])]
#[case("simultaneous", vec![0,1,3,1,2], vec![0,1,3,1,2], vec!["-02"])]
#[case("unbounded", vec![1,0,1,2,1], vec![1,0,1,2,1], vec!["-02","-03"])]
fn repeated_casts_require_unambiguous_bounded_alignment(
    #[case] case: &str,
    #[case] left: Vec<i64>,
    #[case] right: Vec<i64>,
    #[case] suffixes: Vec<&str>,
) {
    let dir = tempfile::tempdir().unwrap();
    let paths = inputs(dir.path());
    for (path, sequence) in paths.iter().zip([left, right]) {
        let mut data: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        data["report"]["masterData"]["abilities"] = (0..4)
            .map(|id| json!({"gameID":46376+id,"name":format!("Spell{id}"),"type":"1"}))
            .collect();
        data["events"] = sequence.iter().enumerate().map(|(i,id)| {
            let at = if case=="nearby" && i==2 {4050} else if case=="simultaneous" && i==2 {4000} else {2000+i*2000};
            json!({"timestamp":at,"type":"cast","sourceID":10,"abilityGameID":46376+id,"fight":1})
        }).collect();
        if case == "phase" {
            data["report"]["fights"][0]["phaseTransitions"] = json!([{"id":2,"startTime":5000}]);
        }
        data["collection"]["eventCount"] = sequence.len().into();
        fs::write(path, serde_json::to_vec(&data).unwrap()).unwrap();
    }
    let mut outputs = Vec::new();
    for (index, ordered) in [paths.clone(), paths.iter().rev().cloned().collect()]
        .iter()
        .enumerate()
    {
        let output = dir.path().join(format!("timeline{index}.kdl"));
        cargo_bin_cmd!("btimeline")
            .arg("generate")
            .args(ordered)
            .args(["--mode", "raid", "-o"])
            .arg(&output)
            .assert()
            .success();
        let timeline = fs::read_to_string(&output).unwrap();
        let doc: kdl::KdlDocument = timeline.parse().unwrap();
        let repeats: Vec<_> = doc.nodes()[0]
            .children()
            .unwrap()
            .nodes()
            .iter()
            .filter(|node| node.get("name").and_then(kdl::KdlValue::as_string) == Some("Spell1"))
            .collect();
        assert_eq!(repeats.len(), suffixes.len(), "{case}");
        for (event, suffix) in repeats.iter().zip(&suffixes) {
            assert!(event.get(0).unwrap().as_string().unwrap().ends_with(suffix));
            assert!(
                event.children().is_none(),
                "repeated identities must not resync"
            );
        }
        let evidence: Value =
            serde_json::from_slice(&fs::read(output.with_extension("evidence.json")).unwrap())
                .unwrap();
        assert!(
            !evidence["alignment_diagnostics"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        let diagnostics = evidence["alignment_diagnostics"].as_array().unwrap();
        if case == "ambiguous" {
            assert!(diagnostics.iter().any(|row| {
                row["ambiguous_reference"]
                    .as_array()
                    .is_some_and(|values| values.len() == 2)
            }));
        }
        if case == "context" || case == "unbounded" || case == "simultaneous" {
            assert!(diagnostics.iter().any(|row| {
                row["unrepresented"]
                    .as_array()
                    .is_some_and(|values| !values.is_empty())
            }));
        }
        for event in evidence["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|event| !event["alignment"].is_null())
        {
            assert!(matches!(
                event["alignment"]["method"].as_str(),
                Some("anchor-constrained-multiset-lcs-v2" | "censored-tail-v1")
            ));
            assert_eq!(event["alignment"]["batches"].as_object().unwrap().len(), 2);
        }
        outputs.push(timeline);
    }
    assert_eq!(
        outputs[0], outputs[1],
        "CLI order must not change occurrence labels"
    );
}

fn inputs(directory: &Path) -> Vec<PathBuf> {
    let base: Value =
        serde_json::from_slice(&fs::read(INPUT).expect("read fixture")).expect("parse fixture");
    [[2000,6050,10000],[2500,6551,10501]].iter().enumerate().map(|(index,times)| {
        let mut data = base.clone();
        let code = format!("generation{index}");
        *data.pointer_mut("/report/code").expect("report code") = code.clone().into();
        *data.pointer_mut("/collection/reportCode").expect("collection code") = code.into();
        *data.pointer_mut("/report/masterData/abilities").expect("abilities") = json!([
            {"gameID":46376,"name":"A","type":"1"},
            {"gameID":46377,"name":"B","type":"1"},
            {"gameID":46378,"name":"C","type":"1"}]);
        *data.pointer_mut("/events").expect("events") = times.iter().zip([46376,46377,46378])
            .map(|(at,id)| json!({"timestamp":at,"type":"cast","sourceID":10,"abilityGameID":id,"fight":1})).collect();
        let path = directory.join(format!("pull{index}.json"));
        fs::write(&path,serde_json::to_vec(&data).expect("serialize input")).expect("write input");
        path
    }).collect()
}

#[rstest::rstest]
#[case("raid", true, "4000000F")]
#[case("dungeon", false, "40000010")]
#[case("alliance", false, "zone")]
fn logs_generate_and_replay_without_a_template_or_intermediate_file(
    #[case] mode: &str,
    #[case] combat_start: bool,
    #[case] reset: &str,
) {
    let dir = tempfile::tempdir().unwrap();
    let inputs = inputs(dir.path());
    let output = dir.path().join("timeline.kdl");
    cargo_bin_cmd!("btimeline")
        .arg("generate")
        .args(&inputs)
        .args(["--mode", mode, "-o"])
        .arg(&output)
        .assert()
        .success();
    let timeline = fs::read_to_string(&output).unwrap();
    insta::assert_snapshot!(format!("generated_{mode}"), timeline);
    assert!(!timeline.contains("FFLogs."));
    assert!(!dir.path().join("process.kdl").exists());
    let report = output.with_extension("evidence.json");
    let evidence: Value = serde_json::from_slice(&fs::read(&report).unwrap()).unwrap();
    assert_eq!(evidence["version"], 2);
    assert_eq!(evidence["mode"], mode);
    assert_eq!(evidence["analysis"]["inputs"].as_array().unwrap().len(), 2);
    assert_eq!(
        evidence["analysis"]["inputs"][0]["observations"][0]["indices"][0],
        0
    );
    let hash: String = ring::digest::digest(&ring::digest::SHA256, timeline.as_bytes())
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    assert_eq!(evidence["timeline_sha256"], hash);
    let expected_b = if combat_start { 5050 } else { 4050 };
    assert_eq!(evidence["events"][1]["median_offset_ms"], expected_b);
    assert_eq!(evidence["events"][1]["rounding_delta_ms"], 50);
    let recording = dir.path().join("recording.json");
    let start = if combat_start {
        json!({"type":"InCombat","fields":{"inGameCombat":"1"}})
    } else {
        json!({"type":"Ability","fields":{"id":"B528","source":"Synthetic Boss"}})
    };
    let mut first = start.clone();
    first["at_ms"] = 0.into();
    let mut second = start;
    second["at_ms"] = 13000.into();
    let reset = if reset == "zone" {
        json!({"at_ms":12000,"type":"ChangeZone","fields":{}})
    } else {
        json!({"at_ms":12000,"type":"ActorControl","fields":{"command":reset}})
    };
    let corrected_b = if combat_start { 5200 } else { 4200 };
    fs::write(
        &recording,
        serde_json::to_vec(&json!({"version":1,"end_ms":24000,"observations":[first,
        {"at_ms":corrected_b,"type":"Ability","fields":{"id":"B529","source":"Synthetic Boss"}},reset,second]}))
        .unwrap(),
    )
    .unwrap();
    // Only the generated KDL participates in replay; provenance remains optional for execution.
    fs::remove_file(report).unwrap();
    cargo_bin_cmd!("btimeline")
        .arg("validate")
        .arg(&output)
        .assert()
        .success();
    let replay = cargo_bin_cmd!("btimeline")
        .arg("compare")
        .arg(&output)
        .arg(&recording)
        .arg("--json")
        .assert()
        .success();
    let result: Value = serde_json::from_slice(&replay.get_output().stdout).unwrap();
    assert_eq!(
        result["summaries"][recording.to_str().unwrap()]["started_runs"],
        2
    );
    let records = result["recordings"][recording.to_str().unwrap()]
        .as_array()
        .unwrap();
    let displayed: Vec<_> = records
        .iter()
        .filter(|record| record["action"] == "event")
        .collect();
    assert_eq!(displayed.len(), 6);
    assert_eq!(
        displayed[1]["at_ms"],
        if combat_start { 5100 } else { 4100 }
    );
    assert_eq!(
        displayed[2]["at_ms"],
        if combat_start { 9100 } else { 8100 }
    );
    assert!(
        records
            .iter()
            .any(|record| record["action"] == "sync" && record["at_ms"] == corrected_b)
    );
    assert_eq!(
        displayed[4]["at_ms"],
        if combat_start { 18100 } else { 17100 }
    );
}

#[rstest::rstest]
#[case("invalid-mode")]
#[case("incomplete-input")]
#[case("mixed-encounters")]
#[case("duplicate-input")]
#[case("intermediate-graph")]
#[case("missing-boss-metadata")]
fn invalid_generation_inputs_leave_no_outputs(#[case] problem: &str) {
    let dir = tempfile::tempdir().unwrap();
    let mut paths = inputs(dir.path());
    let output = dir.path().join("timeline.kdl");
    let mut data: Value = serde_json::from_slice(&fs::read(&paths[1]).unwrap()).unwrap();
    match problem {
        "incomplete-input" => data["collection"]["complete"] = false.into(),
        "mixed-encounters" => data["report"]["fights"][0]["encounterID"] = 99198.into(),
        "duplicate-input" => paths[1] = paths[0].clone(),
        _ => (),
    }
    if problem != "duplicate-input" {
        fs::write(&paths[1], serde_json::to_vec(&data).unwrap()).unwrap();
    }
    if problem == "intermediate-graph" {
        fs::write(&paths[0], "process 1 algorithm=adjacent-batches {}\n").unwrap();
    }
    if problem == "missing-boss-metadata" {
        for path in &paths {
            let mut data: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
            data["report"]["masterData"]["actors"][0]["subType"] = "NPC".into();
            fs::write(path, serde_json::to_vec(&data).unwrap()).unwrap();
        }
    }
    let mode = if problem == "invalid-mode" {
        "unknown"
    } else if problem == "missing-boss-metadata" {
        "dungeon"
    } else {
        "raid"
    };
    let result = cargo_bin_cmd!("btimeline")
        .arg("generate")
        .args(&paths)
        .args(["--mode", mode, "-o"])
        .arg(&output)
        .assert()
        .failure();
    assert!(result.get_output().stdout.is_empty());
    assert!(!output.exists());
    assert!(!output.with_extension("evidence.json").exists());
}

#[test]
fn failed_publication_preserves_inputs_and_existing_evidence() {
    let dir = tempfile::tempdir().unwrap();
    let paths = inputs(dir.path());
    let before: Vec<_> = paths.iter().map(|path| fs::read(path).unwrap()).collect();
    let output = dir.path().join("timeline.kdl");
    let report = output.with_extension("evidence.json");
    fs::write(&report, "previous evidence").unwrap();
    cargo_bin_cmd!("btimeline")
        .arg("generate")
        .args(&paths)
        .args(["--mode", "raid", "-o"])
        .arg(&output)
        .assert()
        .failure();
    assert!(!output.exists());
    assert_eq!(fs::read_to_string(report).unwrap(), "previous evidence");
    assert_eq!(
        paths
            .iter()
            .map(|path| fs::read(path).unwrap())
            .collect::<Vec<_>>(),
        before
    );
}

#[test]
fn mode_is_the_only_required_generation_policy() {
    let help = cargo_bin_cmd!("btimeline")
        .args(["generate", "--help"])
        .assert()
        .success();
    let text = String::from_utf8_lossy(&help.get_output().stdout);
    assert!(text.contains("--mode"));
    assert!(!text.contains("--template"));
    assert!(!text.contains("--holdout"));
}

#[test]
fn boss_origin_alternatives_are_inferred_without_dropping_close_opening_casts() {
    let dir = tempfile::tempdir().unwrap();
    let paths = inputs(dir.path());
    for (path, times) in paths.iter().zip([[2000, 2001], [2501, 2500]]) {
        let mut data: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        data["events"][0]["timestamp"] = times[0].into();
        data["events"][1]["timestamp"] = times[1].into();
        fs::write(path, serde_json::to_vec(&data).unwrap()).unwrap();
    }
    let output = dir.path().join("timeline.kdl");
    cargo_bin_cmd!("btimeline")
        .arg("generate")
        .args(&paths)
        .args(["--mode", "dungeon", "-o"])
        .arg(&output)
        .assert()
        .success();
    let doc: kdl::KdlDocument = fs::read_to_string(output).unwrap().parse().unwrap();
    let nodes = doc.nodes()[0].children().unwrap().nodes();
    assert_eq!(
        nodes
            .iter()
            .filter(|node| node.name().value() == "start")
            .count(),
        2
    );
    let nodes = nodes
        .iter()
        .find(|node| node.name().value() == "phase")
        .unwrap()
        .children()
        .unwrap()
        .nodes();
    assert_eq!(
        nodes
            .iter()
            .filter(|node| node.name().value() == "event")
            .count(),
        3
    );
}

#[test]
fn sync_windows_cover_observed_timing_spread() {
    let dir = tempfile::tempdir().unwrap();
    let paths = inputs(dir.path());
    let mut data: Value = serde_json::from_slice(&fs::read(&paths[1]).unwrap()).unwrap();
    data["events"][1]["timestamp"] = 12051.into();
    data["events"][2]["timestamp"] = 18001.into();
    fs::write(&paths[1], serde_json::to_vec(&data).unwrap()).unwrap();
    let output = dir.path().join("timeline.kdl");
    cargo_bin_cmd!("btimeline")
        .arg("generate")
        .args(&paths)
        .args(["--mode", "raid", "-o"])
        .arg(&output)
        .assert()
        .success();
    let doc: kdl::KdlDocument = fs::read_to_string(output).unwrap().parse().unwrap();
    let event = doc.nodes()[0]
        .children()
        .unwrap()
        .nodes()
        .iter()
        .find(|node| node.get("name").and_then(kdl::KdlValue::as_string) == Some("B"))
        .unwrap();
    let sync = &event.children().unwrap().nodes()[0];
    assert!(sync.get("after").unwrap().as_float().unwrap() >= 6.0);
}

#[rstest::rstest]
#[case("dungeon")]
#[case("alliance")]
fn boss_scopes_exclude_trash_and_restart_directly_on_a_later_boss(#[case] mode: &str) {
    let dir = tempfile::tempdir().unwrap();
    let paths = inputs(dir.path());
    for (path, times) in paths.iter().zip([[14000, 18000], [15500, 19600]]) {
        let mut data: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        data["report"]["masterData"]["actors"]
            .as_array_mut()
            .unwrap()
            .extend([
                json!({"id":11,"gameID":99002,"name":"Second Boss","type":"NPC","subType":"Boss"}),
                json!({"id":12,"gameID":99003,"name":"Trash","type":"NPC","subType":"NPC"}),
            ]);
        data["report"]["fights"][0]["enemyNPCs"]
            .as_array_mut()
            .unwrap()
            .extend([
                json!({"id":11,"gameID":99002}),
                json!({"id":12,"gameID":99003}),
            ]);
        data["report"]["masterData"]["abilities"]
            .as_array_mut()
            .unwrap()
            .extend([
                json!({"gameID":46379,"name":"D","type":"1"}),
                json!({"gameID":46380,"name":"E","type":"1"}),
                json!({"gameID":46381,"name":"Trash cast","type":"1"}),
            ]);
        data["events"].as_array_mut().unwrap().extend([
            json!({"timestamp":times[0],"type":"cast","sourceID":11,"abilityGameID":46379,"fight":1}),
            json!({"timestamp":times[1],"type":"cast","sourceID":11,"abilityGameID":46380,"fight":1}),
            json!({"timestamp":1100,"type":"cast","sourceID":12,"abilityGameID":46381,"fight":1})]);
        data["collection"]["eventCount"] = 6.into();
        fs::write(path, serde_json::to_vec(&data).unwrap()).unwrap();
    }
    let output = dir.path().join("timeline.kdl");
    cargo_bin_cmd!("btimeline")
        .arg("generate")
        .args(&paths)
        .args(["--mode", mode, "-o"])
        .arg(&output)
        .assert()
        .success();
    let timeline = fs::read_to_string(&output).unwrap();
    assert!(!timeline.contains("Trash"));
    let doc: kdl::KdlDocument = timeline.parse().unwrap();
    let phases: Vec<_> = doc.nodes()[0]
        .children()
        .unwrap()
        .nodes()
        .iter()
        .filter(|node| node.name().value() == "phase")
        .collect();
    assert_eq!(phases.len(), 2);
    assert_eq!(
        phases[0].get(0).unwrap().as_string(),
        Some("synthetic-boss")
    );
    assert_eq!(phases[1].get(0).unwrap().as_string(), Some("second-boss"));
    let recording = dir.path().join("recording.json");
    fs::write(
        &recording,
        serde_json::to_vec(&json!({"version":1,"end_ms":22000,"observations":[
            {"at_ms":0,"type":"Ability","fields":{"id":"B528","source":"Synthetic Boss"}},
            {"at_ms":10000,"type":"Ability","fields":{"id":"B52B","source":"Second Boss"}},
            {"at_ms":16000,"type":"ActorControl","fields":{"command":"4000000F"}},
            {"at_ms":17000,"type":"Ability","fields":{"id":"B52B","source":"Second Boss"}}
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
    let records = result["recordings"][recording.to_str().unwrap()]
        .as_array()
        .unwrap();
    let restarted: Vec<_> = records
        .iter()
        .filter(|record| record["run"] == 2 && record["action"] == "event")
        .collect();
    assert_eq!(restarted.len(), 2);
    assert_eq!(restarted[0]["at_ms"], 17000);
    assert_eq!(restarted[1]["at_ms"], 21000);
    assert!(
        restarted
            .iter()
            .all(|record| record["detail"].as_str().unwrap().contains("second-boss"))
    );
}
