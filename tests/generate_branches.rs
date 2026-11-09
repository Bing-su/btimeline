use std::fs;
use std::path::{Path, PathBuf};

use assert_cmd::cargo::cargo_bin_cmd;
use serde_json::{Value, json};

fn inputs(dir: &Path) -> Vec<PathBuf> {
    [vec![0,1,2,3],vec![0,2,1,3],vec![0,1,2,3]].iter().enumerate().map(|(index,sequence)| {
        let mut data:Value=serde_json::from_str(include_str!("fixtures/pull.json")).expect("fixture");
        for (pointer,value) in [
            ("/report/code",format!("choice{index}").into()),("/collection/reportCode",format!("choice{index}").into()),
            ("/report/masterData/abilities",(0..4).map(|id|json!({"gameID":46376+id,"name":format!("Spell{id}"),"type":"1"})).collect()),
            ("/events",sequence.iter().zip([2000,5000,9000,16000]).map(|(id,at)|json!({"timestamp":at,"type":"cast","sourceID":10,"abilityGameID":46376+id,"fight":1})).collect()),
            ("/collection/eventCount",4.into()),
        ] {*data.pointer_mut(pointer).expect("fixture field")=value;}
        let path=dir.join(format!("pull{index}.json"));
        fs::write(&path,serde_json::to_vec(&data).expect("serialize")).expect("write input");
        path
    }).collect()
}

fn observation(at: i64, id: &str) -> Value {
    json!({"at_ms":at,"type":"Ability","fields":{"id":id,"source":"Synthetic Boss"}})
}

#[rstest::rstest]
#[case("left",vec!["Spell0","Spell1","Spell2","Spell3"])]
#[case("right",vec!["Spell0","Spell2","Spell1","Spell3"])]
#[case("missing",vec!["Spell0","Spell3"])]
#[case("restart",vec!["Spell0","Spell1","Spell2","Spell0","Spell2","Spell1","Spell3"])]
fn local_order_choices_replay_without_the_evidence(
    #[case] scenario: &str,
    #[case] names: Vec<&str>,
) {
    let dir = tempfile::tempdir().unwrap();
    let paths = inputs(dir.path());
    let output = dir.path().join("timeline.kdl");
    cargo_bin_cmd!("btimeline")
        .arg("generate")
        .args(&paths)
        .args(["--mode", "raid", "-o"])
        .arg(&output)
        .assert()
        .success();
    let timeline = fs::read_to_string(&output).unwrap();
    if scenario == "left" {
        insta::assert_snapshot!("generated_local_order_branch", timeline);
    }
    let evidence: Value =
        serde_json::from_slice(&fs::read(output.with_extension("evidence.json")).unwrap()).unwrap();
    assert_eq!(evidence["generation_method"], "representative-route-v1");
    assert!(
        evidence["draft"]["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["method"] == "representative-order-choice-v1"
                && row["status"] == "learned")
    );
    assert!(
        evidence["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|event| event["alignment"]["method"] == "representative-order-option-v1")
    );
    let mut observations = vec![
        json!({"at_ms":0,"type":"InCombat","fields":{"inGameCombat":"1"}}),
        observation(1000, "B528"),
    ];
    if scenario != "missing" {
        let (first, second) = if scenario == "right" {
            ("B52A", "B529")
        } else {
            ("B529", "B52A")
        };
        observations.extend([observation(4000, first), observation(8000, second)]);
    }
    if scenario == "missing" {
        observations.push(observation(8000, "B529"));
    }
    let end = if scenario == "restart" {
        observations.extend([
            json!({"at_ms":11000,"type":"ActorControl","fields":{"command":"4000000F"}}),
            json!({"at_ms":20000,"type":"InCombat","fields":{"inGameCombat":"1"}}),
            observation(21000, "B528"),
            observation(24000, "B52A"),
            observation(28000, "B529"),
            observation(35000, "B52B"),
        ]);
        36000
    } else {
        observations.push(observation(15000, "B52B"));
        16000
    };
    let recording = dir.path().join("recording.json");
    fs::write(
        &recording,
        serde_json::to_vec(&json!({"version":1,"end_ms":end,"observations":observations})).unwrap(),
    )
    .unwrap();
    fs::remove_file(output.with_extension("evidence.json")).unwrap();
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
    let displayed: Vec<_> = records
        .iter()
        .filter(|row| row["action"] == "event")
        .collect();
    assert_eq!(displayed.len(), names.len());
    for (row, name) in displayed.iter().zip(names) {
        assert!(row["detail"].as_str().unwrap().contains(name));
    }
    assert!(
        !records
            .iter()
            .any(|row| row["action"] == "ambiguous" || row["action"] == "violation")
    );
    assert_eq!(
        records.iter().any(|row| row["action"] == "unresolved"),
        scenario == "missing"
    );
}

#[test]
fn later_correlated_choices_are_not_generated_as_independent_combinations() {
    let dir = tempfile::tempdir().unwrap();
    let paths = inputs(dir.path());
    for (index, path) in paths.iter().enumerate() {
        let mut data: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        let sequence = if index == 1 {
            vec![0, 2, 1, 3, 5, 4, 6]
        } else {
            vec![0, 1, 2, 3, 4, 5, 6]
        };
        data["report"]["masterData"]["abilities"] = (0..7)
            .map(|id| json!({"gameID":46376+id,"name":format!("Spell{id}"),"type":"1"}))
            .collect();
        data["events"]=sequence.iter().enumerate().map(|(i,id)|json!({"timestamp":2000+i*2000,"type":"cast","sourceID":10,"abilityGameID":46376+id,"fight":1})).collect();
        data["collection"]["eventCount"] = 7.into();
        fs::write(path, serde_json::to_vec(&data).unwrap()).unwrap();
    }
    let output = dir.path().join("timeline.kdl");
    cargo_bin_cmd!("btimeline")
        .arg("generate")
        .args(&paths)
        .args(["--mode", "raid", "-o"])
        .arg(&output)
        .assert()
        .success();
    let doc: kdl::KdlDocument = fs::read_to_string(&output).unwrap().parse().unwrap();
    let root = doc.nodes()[0].children().unwrap();
    let phase = root
        .nodes()
        .iter()
        .find(|node| node.name().value() == "phase")
        .unwrap();
    assert_eq!(
        phase
            .children()
            .unwrap()
            .get("step")
            .unwrap()
            .children()
            .unwrap()
            .nodes()
            .iter()
            .filter(|node| node.name().value() == "branch")
            .count(),
        1
    );
    let report: Value =
        serde_json::from_slice(&fs::read(output.with_extension("evidence.json")).unwrap()).unwrap();
    let choices = report["draft"]["routes"][0]["choices"].as_array().unwrap();
    assert_eq!(choices.len(), 1);
    let paths: Vec<Vec<i64>> = choices[0]["options"]
        .as_array()
        .unwrap()
        .iter()
        .map(|option| {
            option["nodes"]
                .as_array()
                .unwrap()
                .iter()
                .map(|node| node["activity"]["identifier"]["Ability"].as_i64().unwrap())
                .collect()
        })
        .collect();
    assert_eq!(
        paths,
        [
            vec![46377, 46378, 46379, 46380, 46381],
            vec![46378, 46377, 46379, 46381, 46380]
        ]
    );
    assert_eq!(report["events"].as_array().unwrap().len(), 12);
}

#[test]
fn a_missing_cast_is_not_promoted_to_a_choice() {
    let dir = tempfile::tempdir().unwrap();
    let paths = inputs(dir.path());
    let mut data: Value = serde_json::from_slice(&fs::read(&paths[1]).unwrap()).unwrap();
    data["events"]=[(2000,46376),(5000,46377),(16000,46379)].iter().map(|(at,id)|json!({"timestamp":at,"type":"cast","sourceID":10,"abilityGameID":id,"fight":1})).collect();
    data["collection"]["eventCount"] = 3.into();
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
    assert!(
        doc.nodes()[0]
            .children()
            .unwrap()
            .nodes()
            .iter()
            .all(|node| node.name().value() != "phase")
    );
}
