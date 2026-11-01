use std::fs;

use assert_cmd::cargo::cargo_bin_cmd;
use serde_json::{Value, json};

#[rstest::rstest]
#[case("raid")]
#[case("dungeon")]
fn occurrence_graph_keeps_unprinted_helpers_and_binds_timing_samples(#[case] mode: &str) {
    let dir = tempfile::tempdir().unwrap();
    let mut paths = Vec::new();
    for peer in 0..2 {
        let mut data: Value = serde_json::from_str(include_str!("fixtures/pull.json")).unwrap();
        let code = format!("progression{peer}");
        data["report"]["code"] = code.clone().into();
        data["collection"]["reportCode"] = code.into();
        data["report"]["masterData"]["actors"]
            .as_array_mut()
            .unwrap()
            .push(json!({"id":11,"gameID":99002,"name":"Helper","type":"NPC","subType":"NPC"}));
        data["report"]["fights"][0]["enemyNPCs"]
            .as_array_mut()
            .unwrap()
            .push(json!({"id":11,"gameID":99002}));
        data["report"]["masterData"]["abilities"] = json!([
            {"gameID":100,"name":"Opening","type":"1"},
            {"gameID":200,"name":"Pulse","type":"1"},
            {"gameID":300,"name":"Continuation","type":"1"}
        ]);
        data["events"] = [(2000,100,10),(4000,200,11),(6000,200,11),(8000,200,11),(10000,300,10)].iter()
            .map(|(at,id,actor)|json!({"timestamp":at,"type":"cast","sourceID":actor,"abilityGameID":id,"fight":1})).collect();
        data["collection"]["eventCount"] = 5.into();
        let path = dir.path().join(format!("{peer}.json"));
        fs::write(&path, serde_json::to_vec(&data).unwrap()).unwrap();
        paths.push(path);
    }
    let output = dir.path().join("timeline.kdl");
    cargo_bin_cmd!("btimeline")
        .arg("generate")
        .args(paths)
        .args(["--mode", mode, "-o"])
        .arg(&output)
        .assert()
        .success();
    let evidence: Value =
        serde_json::from_slice(&fs::read(output.with_extension("evidence.json")).unwrap()).unwrap();
    let traces = evidence["progression"]["traces"].as_array().unwrap();
    assert_eq!(traces.len(), 2);
    for trace in traces {
        assert_eq!(trace["nodes"].as_array().unwrap().len(), 5);
        assert_eq!(trace["relations_complete"], true);
        let helper: Vec<_> = trace["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|node| node["activity"]["source"] == "Helper")
            .collect();
        assert_eq!(helper.len(), 3);
        assert_eq!(
            helper
                .iter()
                .map(|node| node["occurrence"].as_u64().unwrap())
                .collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        assert!(
            helper
                .iter()
                .all(|node| node.get("rounded_offset_ms").is_none())
        );
    }
    // Existing lowering may exclude the helper; its raw occurrences must still exist in the graph.
    assert!(
        evidence["events"]
            .as_array()
            .unwrap()
            .iter()
            .all(|event| event["activity"]["source"] != "Helper")
    );
    for event in evidence["events"].as_array().unwrap() {
        for sample in event["samples"].as_array().unwrap() {
            let trace = traces
                .iter()
                .find(|trace| trace["trace"] == sample["trace"])
                .unwrap();
            let node = &trace["nodes"][event["observation_nodes"][sample["trace"].as_str().unwrap()]
                .as_u64()
                .unwrap() as usize];
            assert_eq!(node["activity"], event["activity"]);
            let input = evidence["analysis"]["inputs"]
                .as_array()
                .unwrap()
                .iter()
                .find(|input| input["trace"] == sample["trace"])
                .unwrap();
            assert_eq!(
                input["observations"][node["observation"].as_u64().unwrap() as usize]["at_ms"],
                sample["at_ms"]
            );
        }
    }
}
