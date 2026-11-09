use std::fs;
use std::path::{Path, PathBuf};

use assert_cmd::cargo::cargo_bin_cmd;
use serde_json::{Value, json};

fn input(
    dir: &Path,
    peer: usize,
    kind: &str,
    owned: bool,
    listed: bool,
    same_name: bool,
) -> PathBuf {
    let mut data: Value =
        serde_json::from_str(include_str!("fixtures/pull.json")).expect("fixture");
    let name = if same_name {
        "Synthetic Boss"
    } else {
        "Unlisted NPC"
    };
    let actor = json!({"id":11,"gameID":99002,"name":name,"type":kind,"subType":"NPC","petOwner":if owned {json!(1)} else {Value::Null}});
    data.pointer_mut("/report/masterData/actors")
        .and_then(Value::as_array_mut)
        .expect("actors")
        .push(actor);
    if owned {
        data.pointer_mut("/report/masterData/actors").and_then(Value::as_array_mut).expect("actors")
            .push(json!({"id":1,"gameID":1,"name":"Party Member","type":"Player","subType":"Unknown"}));
    }
    if listed {
        data.pointer_mut("/report/fights/0/enemyNPCs")
            .and_then(Value::as_array_mut)
            .expect("enemies")
            .push(json!({"id":11,"gameID":99002}));
    }
    let mut events = vec![
        json!({"timestamp":2000,"type":"cast","sourceID":10,"abilityGameID":100,"fight":1}),
        json!({"timestamp":3500,"type":"begincast","sourceID":11,"abilityGameID":400,"fight":1}),
        json!({"timestamp":4000,"type":"cast","sourceID":11,"abilityGameID":400,"fight":1}),
        json!({"timestamp":6000,"type":"cast","sourceID":10,"abilityGameID":300,"fight":1}),
        json!({"timestamp":10000,"type":"cast","sourceID":10,"abilityGameID":500,"fight":1}),
    ];
    if same_name {
        events.push(
            json!({"timestamp":2000,"type":"cast","sourceID":11,"abilityGameID":100,"fight":1}),
        );
    }
    for (pointer, value) in [
        ("/report/code", json!(format!("unclassified{peer}"))),
        (
            "/collection/reportCode",
            json!(format!("unclassified{peer}")),
        ),
        (
            "/report/masterData/abilities",
            json!([{"gameID":100,"name":"Opening","type":"1"},{"gameID":300,"name":"Middle","type":"1"},{"gameID":500,"name":"Ending","type":"1"},{"gameID":400,"name":"Candidate","type":"1"}]),
        ),
        ("/collection/eventCount", json!(events.len())),
        ("/events", json!(events)),
    ] {
        *data.pointer_mut(pointer).expect("field") = value;
    }
    let path = dir.join(format!("{peer}.json"));
    fs::write(&path, serde_json::to_vec(&data).expect("encode fixture")).expect("write fixture");
    path
}

#[rstest::rstest]
#[case("raid", "NPC", false, false, false, true, false)]
#[case("dungeon", "NPC", false, false, false, true, false)]
#[case("raid", "NPC", true, false, false, false, false)]
#[case("raid", "Player", false, false, false, false, false)]
#[case("raid", "NPC", false, true, false, true, true)]
#[case("raid", "NPC", false, false, true, true, false)]
#[case("dungeon", "NPC", false, false, true, true, false)]
fn unlisted_npc_casts_are_evidence_without_automatic_execution(
    #[case] mode: &str,
    #[case] kind: &str,
    #[case] owned: bool,
    #[case] listed: bool,
    #[case] same_name: bool,
    #[case] retained: bool,
    #[case] executable: bool,
) {
    let dir = tempfile::tempdir().unwrap();
    let paths: Vec<_> = (0..3)
        .map(|peer| input(dir.path(), peer, kind, owned, listed, same_name))
        .collect();
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
    for trace in evidence["progression"]["traces"].as_array().unwrap() {
        let nodes: Vec<_> = trace["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|node| node["activity"]["identifier"]["Ability"] == 400)
            .collect();
        assert_eq!(nodes.len(), usize::from(retained));
        if retained {
            assert_eq!(nodes[0]["unclassified"], !listed);
        }
    }
    let rows = evidence["events"].as_array().unwrap();
    assert_eq!(rows.len(), if executable { 4 } else { 3 });
    assert_eq!(
        rows.iter()
            .any(|row| row["activity"]["identifier"]["Ability"] == 400),
        executable
    );
    if same_name {
        let first = &evidence["progression"]["traces"][0]["nodes"][0];
        assert_eq!(first["activity"]["identifier"]["Ability"], 100);
        assert_eq!(first["unclassified"], false);
    }
    if retained && !listed {
        for input in evidence["analysis"]["inputs"].as_array().unwrap() {
            let indices: Vec<_> = input["observations"]
                .as_array()
                .unwrap()
                .iter()
                .flat_map(|batch| {
                    batch["unclassified_indices"]
                        .as_array()
                        .into_iter()
                        .flatten()
                })
                .collect();
            assert_eq!(indices.len(), if same_name { 3 } else { 2 });
        }
        let clean: Vec<_> = paths
            .iter()
            .enumerate()
            .map(|(peer, path)| {
                let mut data: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
                data["events"]
                    .as_array_mut()
                    .unwrap()
                    .retain(|event| event["sourceID"] != 11);
                data["collection"]["eventCount"] = data["events"].as_array().unwrap().len().into();
                let path = dir.path().join(format!("clean-{peer}.json"));
                fs::write(&path, serde_json::to_vec(&data).unwrap()).unwrap();
                path
            })
            .collect();
        let clean_output = dir.path().join("clean.kdl");
        cargo_bin_cmd!("btimeline")
            .arg("generate")
            .args(clean)
            .args(["--mode", mode, "-o"])
            .arg(&clean_output)
            .assert()
            .success();
        assert_eq!(
            fs::read_to_string(&output).unwrap(),
            fs::read_to_string(clean_output).unwrap()
        );
        let process = dir.path().join("process.kdl");
        cargo_bin_cmd!("btimeline")
            .arg("mine")
            .args(&paths)
            .arg("-o")
            .arg(&process)
            .assert()
            .success();
        let doc: kdl::KdlDocument = fs::read_to_string(process).unwrap().parse().unwrap();
        let children = doc.nodes()[0].children().unwrap();
        let candidate = children
            .nodes()
            .iter()
            .find(|node| {
                node.name().value() == "activity"
                    && node
                        .children()
                        .and_then(|children| children.nodes().first())
                        .and_then(|signal| signal.get("name"))
                        .and_then(kdl::KdlValue::as_string)
                        == Some("Candidate")
            })
            .unwrap();
        let id = candidate.get(0).unwrap();
        let common = children
            .nodes()
            .iter()
            .find(|node| node.name().value() == "common-progress")
            .unwrap();
        assert!(
            !common
                .children()
                .unwrap()
                .nodes()
                .iter()
                .any(|node| node.get(0) == Some(id))
        );
        assert!(children.to_string().contains("unclassified"));
    }
}

#[test]
fn unclassified_same_name_casts_do_not_change_the_core_loop_or_add_syncs() {
    let dir = tempfile::tempdir().unwrap();
    let paths:Vec<_>=(0..3).map(|peer| {
        let path=input(dir.path(),peer,"NPC",false,false,true);
        let mut data:Value=serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        let mut events:Vec<_>=(0..9).map(|index|json!({"timestamp":2000+index*2000,"type":"cast","sourceID":10,"abilityGameID":([100,300,500][index%3]),"fight":1})).collect();
        for at in [7000,13000] {events.push(json!({"timestamp":at,"type":"cast","sourceID":11,"abilityGameID":400,"fight":1}));}
        data["collection"]["eventCount"]=events.len().into(); data["events"]=events.into();
        data["report"]["masterData"]["actors"][1]["subType"]="Boss".into();
        fs::write(&path,serde_json::to_vec(&data).unwrap()).unwrap();path
    }).collect();
    let output = dir.path().join("loop.kdl");
    cargo_bin_cmd!("btimeline")
        .arg("generate")
        .args(paths)
        .args(["--mode", "dungeon", "-o"])
        .arg(&output)
        .assert()
        .success();
    let evidence: Value =
        serde_json::from_slice(&fs::read(output.with_extension("evidence.json")).unwrap()).unwrap();
    let loop_ = &evidence["loops"][0];
    assert_eq!(loop_["status"], "rendered");
    assert_eq!(loop_["model"]["body"].as_array().unwrap().len(), 3);
    let events = loop_["events"].as_array().unwrap();
    assert_eq!(events.len(), 4);
    let helper = events
        .iter()
        .find(|event| event["helper_association"].is_object())
        .unwrap();
    assert_eq!(helper["sync"], false);
    assert_eq!(helper["activities"][0]["identifier"]["Ability"], 400);
    let document: kdl::KdlDocument = fs::read_to_string(&output).unwrap().parse().unwrap();
    let phase = &document
        .get("timeline")
        .unwrap()
        .children()
        .unwrap()
        .nodes()
        .iter()
        .find(|node| node.name().value() == "phase")
        .unwrap();
    let body = phase
        .children()
        .unwrap()
        .get("step")
        .unwrap()
        .children()
        .unwrap()
        .get("loop")
        .unwrap()
        .children()
        .unwrap();
    let display = body
        .nodes()
        .iter()
        .find(|node| {
            node.get(0).and_then(kdl::KdlValue::as_string)
                == helper["event"]
                    .as_str()
                    .and_then(|id| id.rsplit('/').next())
        })
        .unwrap();
    assert!(display.children().is_none());
    for trace in evidence["progression"]["traces"].as_array().unwrap() {
        assert_eq!(
            trace["nodes"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|node| node["unclassified"] == true)
                .count(),
            2
        );
    }
    assert!(
        evidence["excluded"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row["activity"]["identifier"]["Ability"] != 400)
    );
}

#[rstest::rstest]
#[case(false)]
#[case(true)]
fn invalid_unclassified_casts_do_not_produce_partial_output(#[case] missing_id: bool) {
    let dir = tempfile::tempdir().unwrap();
    let paths: Vec<_> = (0..2)
        .map(|peer| input(dir.path(), peer, "NPC", false, false, false))
        .collect();
    let mut data: Value = serde_json::from_slice(&fs::read(&paths[0]).unwrap()).unwrap();
    if missing_id {
        data["events"][1]
            .as_object_mut()
            .unwrap()
            .remove("abilityGameID");
    } else {
        data["events"][1]["abilityGameID"] = 999.into();
    }
    fs::write(&paths[0], serde_json::to_vec(&data).unwrap()).unwrap();
    let output = dir.path().join("invalid.kdl");
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

#[rstest::rstest]
#[case(false, false)]
#[case(true, false)]
#[case(true, true)]
fn unnamed_npc_casts_remain_unclassified_without_fabricated_names(
    #[case] listed: bool,
    #[case] owned: bool,
) {
    let dir = tempfile::tempdir().unwrap();
    let paths: Vec<_> = (0..2)
        .map(|peer| {
            let path = input(dir.path(), peer, "NPC", owned, listed, false);
            let mut data: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            data["report"]["masterData"]["actors"][1]["name"] = "".into();
            data["events"].as_array_mut().unwrap().push(json!({"timestamp":5000,"type":"tether","sourceID":11,"targetID":10,"tetherID":123,"fight":1}));
            data["collection"]["eventCount"]=data["events"].as_array().unwrap().len().into();
            fs::write(&path, serde_json::to_vec(&data).unwrap()).unwrap();
            path
        })
        .collect();
    let output = dir.path().join("anonymous.kdl");
    cargo_bin_cmd!("btimeline")
        .arg("generate")
        .args(paths)
        .args(["--mode", "raid", "-o"])
        .arg(&output)
        .assert()
        .success();
    let evidence: Value =
        serde_json::from_slice(&fs::read(output.with_extension("evidence.json")).unwrap()).unwrap();
    for trace in evidence["progression"]["traces"].as_array().unwrap() {
        let candidate = trace["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|node| node["activity"]["identifier"]["Ability"] == 400)
            .unwrap();
        assert_eq!(candidate["unclassified"], true);
        assert!(candidate["activity"]["source"].is_null());
    }
    assert_eq!(evidence["events"].as_array().unwrap().len(), 3);
    assert!(
        evidence["analysis"]["inputs"][0]["observations"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|batch| batch["signals"].as_array().unwrap())
            .any(|signal| signal["kind"] == "tether"
                && signal["source"].is_null()
                && signal["target"] == "Synthetic Boss")
    );
}
