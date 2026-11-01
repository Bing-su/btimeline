use std::fs;

use assert_cmd::cargo::cargo_bin_cmd;
use serde_json::{Value, json};

#[rstest::rstest]
#[case("raid", 45, 45, false, 3)]
#[case("dungeon", 45, 45, false, 3)]
#[case("alliance", 45, 45, false, 3)]
#[case("raid", 0, 45, false, 2)]
#[case("raid", 45, 0, false, 2)]
#[case("raid", 0, 0, false, 1)]
#[case("raid", 45, 45, true, 0)]
fn boundary_uncertainty_does_not_discard_displayable_interior(
    #[case] mode: &str,
    #[case] after: i64,
    #[case] before: i64,
    #[case] phase: bool,
    #[case] count: usize,
) {
    let dir = tempfile::tempdir().unwrap();
    let mut paths = Vec::new();
    for peer in 0..2 {
        let mut data: Value = serde_json::from_str(include_str!("fixtures/pull.json")).unwrap();
        let code = format!("boundary{peer}");
        data["report"]["code"] = code.clone().into();
        data["collection"]["reportCode"] = code.into();
        data["report"]["masterData"]["abilities"] = (0..3)
            .map(|id| json!({"gameID":100+id,"name":format!("Spell{id}"),"type":"1"}))
            .collect();
        let records = [
            (2000, 100),
            (2000 + after, 101),
            (6000, 101),
            (10000 - before, 101),
            (10000, 102),
        ];
        data["events"] = records.iter().map(|(at,id)|json!({"timestamp":at,"type":"cast","sourceID":10,"abilityGameID":id,"fight":1})).collect();
        data["collection"]["eventCount"] = records.len().into();
        if phase {
            data["report"]["fights"][0]["phaseTransitions"] = json!([{"id":2,"startTime":5000}]);
        }
        let path = dir.path().join(format!("{peer}.json"));
        fs::write(&path, serde_json::to_vec(&data).unwrap()).unwrap();
        paths.push(path);
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
    let interior: Vec<_> = evidence["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|event| event["activity"]["identifier"]["Ability"] == 101)
        .collect();
    assert_eq!(interior.len(), count);
    for event in interior {
        assert_eq!(
            event["alignment"]["bounds"]["boundary_policy"],
            "display-interior-v1"
        );
        assert_eq!(event["samples"].as_array().unwrap().len(), 2);
    }
    let doc: kdl::KdlDocument = fs::read_to_string(output).unwrap().parse().unwrap();
    let body = doc.nodes()[0].children().unwrap();
    let phase = body
        .nodes()
        .iter()
        .find(|node| node.name().value() == "phase");
    let rows = phase.map_or(body, |node| node.children().unwrap()).nodes();
    // Matching a display event near a bound must never grant that repeated identity a clock sync.
    assert!(
        rows.iter()
            .filter(|node| node.get("name").and_then(kdl::KdlValue::as_string) == Some("Spell1"))
            .all(|node| node.children().is_none())
    );
}

#[rstest::rstest]
#[case("raid", 2, vec![4000], 1)]
#[case("dungeon", 8, vec![4000,8000], 2)]
#[case("alliance", 3, vec![4000,4050], 0)]
fn exact_timestamp_waves_preserve_entities_and_separate_times(
    #[case] mode: &str,
    #[case] instances: usize,
    #[case] times: Vec<i64>,
    #[case] expected: usize,
) {
    let dir = tempfile::tempdir().unwrap();
    let mut paths = Vec::new();
    for peer in 0..2 {
        let mut data: Value = serde_json::from_str(include_str!("fixtures/pull.json")).unwrap();
        let code = format!("waves{peer}");
        data["report"]["code"] = code.clone().into();
        data["collection"]["reportCode"] = code.into();
        data["report"]["masterData"]["abilities"] = json!([
            {"gameID":101,"name":"Opening","type":"1"},
            {"gameID":202,"name":"Wave","type":"1"},
            {"gameID":303,"name":"Continuation","type":"1"}
        ]);
        let mut events = vec![
            json!({"timestamp":2000,"type":"cast","sourceID":10,"abilityGameID":101,"fight":1}),
        ];
        for at in &times {
            for instance in 1..=instances + peer {
                events.push(json!({"timestamp":at,"type":"cast","sourceID":10,"sourceInstance":instance,"abilityGameID":202,"fight":1}));
            }
        }
        events.push(
            json!({"timestamp":12000,"type":"cast","sourceID":10,"abilityGameID":303,"fight":1}),
        );
        // Packet order and different entity counts cannot change the shared display schedule.
        if peer == 1 {
            events.reverse();
        }
        data["collection"]["eventCount"] = events.len().into();
        data["events"] = events.into();
        let path = dir.path().join(format!("{peer}.json"));
        fs::write(&path, serde_json::to_vec(&data).unwrap()).unwrap();
        paths.push(path);
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
    let waves: Vec<_> = evidence["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|event| event["activity"]["identifier"]["Ability"] == 202)
        .collect();
    assert_eq!(waves.len(), expected);
    for event in waves {
        for peer in 0..2 {
            let id = format!("waves{peer}/1");
            let wave = &event["alignment"]["waves"][&id];
            assert_eq!(wave["instances"], instances + peer);
            let batch = wave["batch"].as_u64().unwrap() as usize;
            let input = evidence["analysis"]["inputs"]
                .as_array()
                .unwrap()
                .iter()
                .find(|input| input["trace"] == id)
                .unwrap();
            let raw = &input["observations"][batch];
            assert_eq!(raw["signals"].as_array().unwrap().len(), instances + peer);
            assert_eq!(raw["indices"].as_array().unwrap().len(), instances + peer);
        }
    }
    if mode == "raid" {
        insta::assert_snapshot!(
            "generated_exact_timestamp_wave",
            fs::read_to_string(output).unwrap()
        );
    }
}
