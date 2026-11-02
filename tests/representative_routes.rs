use std::fs;
use std::path::{Path, PathBuf};

use assert_cmd::cargo::cargo_bin_cmd;
use serde_json::{Value, json};

fn fixture(directory: &Path, code: &str, kill: bool, casts: &[(i64, i64)]) -> PathBuf {
    let mut data: Value =
        serde_json::from_str(include_str!("fixtures/pull.json")).expect("valid test fixture");
    for (pointer, value) in [
        ("/report/code", code.into()),
        ("/collection/reportCode", code.into()),
        ("/report/fights/0/kill", kill.into()),
        ("/report/masterData/abilities", (1..=4).map(|id| json!({"gameID":id,"name":format!("Skill {id}"),"type":"1"})).collect()),
        ("/events", casts.iter().map(|(at,id)| json!({"timestamp":at,"type":"cast","sourceID":10,"abilityGameID":id,"fight":1})).collect()),
        ("/collection/eventCount", casts.len().into()),
    ] {
        *data.pointer_mut(pointer).expect("fixture field") = value;
    }
    let path = directory.join(format!("{code}.json"));
    fs::write(
        &path,
        serde_json::to_vec(&data).expect("valid test fixture"),
    )
    .expect("valid test fixture");
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
        &fs::read(output.with_extension("evidence.json")).expect("valid test fixture"),
    )
    .expect("valid test fixture")
}

#[rstest::rstest]
#[case("raid")]
#[case("dungeon")]
#[case("alliance")]
fn a_single_wipe_preserves_repeated_occurrences_and_runs_without_evidence(#[case] mode: &str) {
    let dir = tempfile::tempdir().expect("valid test fixture");
    let input = fixture(
        dir.path(),
        "wipe",
        false,
        &[(2000, 1), (4000, 2), (6000, 2), (8000, 2), (10000, 3)],
    );
    let output = dir.path().join("timeline.kdl");
    let evidence = generate(&[input], mode, &output);
    assert_eq!(evidence["version"], 3);
    let route = &evidence["draft"]["routes"][0];
    assert_eq!(route["reference"], "wipe/1");
    assert_eq!(route["coverage"], "unconfirmed");
    assert_eq!(route["nodes"].as_array().unwrap().len(), 5);
    assert_eq!(route["before"], json!([[0, 1], [1, 2], [2, 3], [3, 4]]));
    assert_eq!(evidence["events"].as_array().unwrap().len(), 5);
    let timeline = fs::read_to_string(&output).expect("valid test fixture");
    assert!(timeline.contains("skill-2-synthetic-boss-2-03"));
    assert_eq!(
        timeline
            .lines()
            .filter(|line| line.trim_start().starts_with("sync "))
            .count(),
        2
    );
    fs::remove_file(output.with_extension("evidence.json")).expect("valid test fixture");
    let recording = dir.path().join("recording.json");
    let start = if mode == "raid" {
        json!({"at_ms":0,"type":"InCombat","fields":{"inGameCombat":"1"}})
    } else {
        json!({"at_ms":0,"type":"Ability","fields":{"id":"1","source":"Synthetic Boss"}})
    };
    fs::write(
        &recording,
        serde_json::to_vec(&json!({"version":1,"end_ms":10000,"observations":[start]}))
            .expect("valid test fixture"),
    )
    .expect("valid test fixture");
    let replay = cargo_bin_cmd!("btimeline")
        .arg("compare")
        .arg(&output)
        .arg(&recording)
        .arg("--json")
        .assert()
        .success();
    let replay: Value =
        serde_json::from_slice(&replay.get_output().stdout).expect("valid test fixture");
    assert_eq!(
        replay["recordings"][recording.to_str().unwrap()]
            .as_array()
            .unwrap()
            .iter()
            .filter(|record| record["action"] == "event")
            .count(),
        5
    );
}

#[test]
fn representative_uses_observed_progress_instead_of_a_clear_or_silent_tail() {
    let dir = tempfile::tempdir().expect("valid test fixture");
    let paths = vec![
        fixture(
            dir.path(),
            "clear-a",
            true,
            &[(2000, 1), (4000, 2), (6000, 3)],
        ),
        fixture(
            dir.path(),
            "clear-b",
            true,
            &[(2000, 1), (4000, 2), (6000, 3)],
        ),
        fixture(
            dir.path(),
            "long-wipe",
            false,
            &[(2000, 1), (4000, 2), (6000, 2), (10000, 3), (14000, 4)],
        ),
    ];
    let output = dir.path().join("forward.kdl");
    let evidence = generate(&paths, "raid", &output);
    assert_eq!(evidence["draft"]["routes"][0]["reference"], "long-wipe/1");
    assert_eq!(
        evidence["draft"]["routes"][0]["nodes"]
            .as_array()
            .unwrap()
            .len(),
        5
    );
    let reverse = generate(
        &paths.into_iter().rev().collect::<Vec<_>>(),
        "raid",
        &dir.path().join("reverse.kdl"),
    );
    assert_eq!(evidence["draft"], reverse["draft"]);
}

#[test]
fn draft_topology_does_not_depend_on_display_times() {
    let dir = tempfile::tempdir().expect("valid test fixture");
    let input = fixture(
        dir.path(),
        "same",
        false,
        &[(2000, 1), (4000, 2), (4080, 3), (4160, 4)],
    );
    let first = generate(
        std::slice::from_ref(&input),
        "raid",
        &dir.path().join("first.kdl"),
    );
    let input = fixture(
        dir.path(),
        "same",
        false,
        &[(3000, 1), (5000, 2), (5080, 3), (5160, 4)],
    );
    let second = generate(&[input], "raid", &dir.path().join("second.kdl"));
    assert_eq!(first["draft"]["routes"], second["draft"]["routes"]);
    assert_eq!(
        first["draft"]["routes"][0]["before"],
        json!([[0, 1], [0, 2], [1, 3]])
    );
    assert_ne!(
        first["events"][0]["rounded_offset_ms"],
        second["events"][0]["rounded_offset_ms"]
    );
}

#[test]
fn excluding_an_unclassified_wave_preserves_the_remaining_order_and_raw_references() {
    let dir = tempfile::tempdir().expect("temporary directory");
    let input = fixture(
        dir.path(),
        "unclassified",
        false,
        &[(2000, 1), (4000, 2), (6000, 3)],
    );
    let mut data: Value =
        serde_json::from_slice(&fs::read(&input).expect("fixture")).expect("fixture");
    data["report"]["masterData"]["actors"]
        .as_array_mut()
        .expect("actors")
        .push(json!({"id":11,"gameID":99002,"name":"Unknown Helper","type":"NPC","subType":"NPC"}));
    data["events"][1]["sourceID"] = 11.into();
    fs::write(&input, serde_json::to_vec(&data).expect("encode fixture")).expect("save fixture");
    let evidence = generate(&[input], "raid", &dir.path().join("timeline.kdl"));
    let route = &evidence["draft"]["routes"][0];
    assert_eq!(route["nodes"].as_array().expect("route nodes").len(), 2);
    assert_eq!(route["before"], json!([[0, 1]]));
    assert_eq!(route["nodes"][1]["observation_nodes"]["unclassified/1"], 2);
    assert_eq!(
        evidence["progression"]["traces"][0]["nodes"]
            .as_array()
            .expect("raw nodes")
            .len(),
        3
    );
}

#[test]
fn each_boss_selects_its_own_representative_scope() {
    let dir = tempfile::tempdir().expect("temporary directory");
    let paths = vec![
        fixture(
            dir.path(),
            "alpha",
            true,
            &[(2000, 1), (4000, 2), (8000, 3), (10000, 1), (12000, 2)],
        ),
        fixture(
            dir.path(),
            "beta",
            true,
            &[(2000, 1), (4000, 2), (6000, 1), (10000, 2), (14000, 3)],
        ),
    ];
    for (index, path) in paths.iter().enumerate() {
        let mut data: Value =
            serde_json::from_slice(&fs::read(path).expect("fixture")).expect("fixture");
        data["report"]["masterData"]["actors"]
            .as_array_mut()
            .expect("actors")
            .push(
                json!({"id":11,"gameID":99002,"name":"Second Boss","type":"NPC","subType":"Boss"}),
            );
        data["report"]["fights"][0]["enemyNPCs"]
            .as_array_mut()
            .expect("enemies")
            .push(json!({"id":11,"gameID":99002}));
        for event in data["events"]
            .as_array_mut()
            .expect("events")
            .iter_mut()
            .skip(if index == 0 { 3 } else { 2 })
        {
            event["sourceID"] = 11.into();
        }
        fs::write(path, serde_json::to_vec(&data).expect("encode fixture")).expect("save fixture");
    }
    let evidence = generate(&paths, "dungeon", &dir.path().join("timeline.kdl"));
    let routes = evidence["draft"]["routes"].as_array().expect("routes");
    let references: Vec<_> = routes
        .iter()
        .map(|route| (route["source"].as_str(), route["reference"].as_str()))
        .collect();
    assert_eq!(
        references,
        [
            (Some("Second Boss"), Some("beta/1")),
            (Some("Synthetic Boss"), Some("alpha/1"))
        ]
    );
}

#[test]
fn overlapping_boss_entities_are_not_treated_as_one_supported_route() {
    let dir = tempfile::tempdir().expect("temporary directory");
    let input = fixture(
        dir.path(),
        "overlap",
        false,
        &[(2000, 1), (4000, 2), (6000, 3), (8000, 4)],
    );
    let mut data: Value =
        serde_json::from_slice(&fs::read(&input).expect("fixture")).expect("fixture");
    for (index, event) in data["events"]
        .as_array_mut()
        .expect("events")
        .iter_mut()
        .enumerate()
    {
        event["sourceInstance"] = (1 + index % 2).into();
    }
    fs::write(&input, serde_json::to_vec(&data).expect("encode fixture")).expect("save fixture");
    let output = dir.path().join("timeline.kdl");
    cargo_bin_cmd!("btimeline")
        .arg("generate")
        .arg(input)
        .args(["--mode", "dungeon", "-o"])
        .arg(&output)
        .assert()
        .failure();
    assert!(!output.exists());
    assert!(!output.with_extension("evidence.json").exists());
}

#[rstest::rstest]
#[case(false)]
#[case(true)]
fn matching_routes_can_learn_from_wipes_without_two_clears(#[case] first_kill: bool) {
    let dir = tempfile::tempdir().expect("temporary directory");
    let paths = vec![
        fixture(
            dir.path(),
            "alpha",
            first_kill,
            &[(2000, 1), (3000, 2), (6000, 3)],
        ),
        fixture(
            dir.path(),
            "beta",
            false,
            &[(2000, 1), (2080, 2), (6000, 3)],
        ),
    ];
    let output = dir.path().join("timeline.kdl");
    let evidence = generate(&paths, "raid", &output);
    let route = &evidence["draft"]["routes"][0];
    assert_eq!(route["reference"], "alpha/1");
    assert_eq!(route["peers_complete"], true);
    // The representative orders A before B; the other pull only confirms both precede C.
    assert_eq!(route["before"], json!([[0, 2], [1, 2]]));
    for node in route["nodes"].as_array().expect("nodes") {
        assert_eq!(
            node["observation_nodes"]
                .as_object()
                .expect("bindings")
                .len(),
            2
        );
    }
    for event in evidence["events"].as_array().expect("events") {
        assert_eq!(event["samples"].as_array().expect("samples").len(), 2);
    }
    let reversed = generate(
        &paths.into_iter().rev().collect::<Vec<_>>(),
        "raid",
        &dir.path().join("reverse.kdl"),
    );
    assert_eq!(evidence["draft"], reversed["draft"]);
    assert_eq!(evidence["events"], reversed["events"]);
}

#[test]
fn divergent_wipe_routes_are_reported_instead_of_silently_picking_one() {
    let dir = tempfile::tempdir().expect("temporary directory");
    let paths = vec![
        fixture(
            dir.path(),
            "alpha",
            false,
            &[(2000, 1), (4000, 2), (6000, 3)],
        ),
        fixture(
            dir.path(),
            "beta",
            false,
            &[(2000, 1), (4000, 3), (6000, 2)],
        ),
    ];
    let output = dir.path().join("timeline.kdl");
    let result = cargo_bin_cmd!("btimeline")
        .arg("generate")
        .args(paths)
        .args(["--mode", "raid", "-o"])
        .arg(&output)
        .assert()
        .failure();
    assert!(
        String::from_utf8_lossy(&result.get_output().stderr)
            .contains("Representative routes differ")
    );
    assert!(!output.exists());
    assert!(!output.with_extension("evidence.json").exists());
}
