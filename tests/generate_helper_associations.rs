use std::fs;
use std::path::{Path, PathBuf};

use assert_cmd::cargo::cargo_bin_cmd;
use serde_json::{Value, json};

fn input(directory: &Path, peer: usize, scenario: &str) -> PathBuf {
    let mut data: Value =
        serde_json::from_str(include_str!("fixtures/pull.json")).expect("fixture");
    let code = if scenario == "shared-report" {
        "shared".to_owned()
    } else {
        format!("helper{peer}")
    };
    let fight = peer + 1;
    let mut actors = vec![
        json!({"id":10,"gameID":99001,"name":"Synthetic Boss","type":"NPC","subType":"Boss"}),
        json!({"id":11,"gameID":99002,"name":if scenario=="anonymous" {""}else{"Effect NPC"},"type":"NPC","subType":"NPC","petOwner":null}),
    ];
    let mut enemies = vec![json!({"id":10,"gameID":99001})];
    let ids = [9, 1, 2, 3, 4, 1, 2, 3, 4, 1, 2, 3];
    let mut events = Vec::new();
    for (index, id) in ids.iter().enumerate() {
        let at = 2000 + index * 1000;
        events.push(
            json!({"timestamp":at,"type":"cast","sourceID":10,"abilityGameID":id,"fight":fight}),
        );
        if *id == 2
            && (scenario != "one-time" || index == 2)
            && !(scenario == "missing" && peer == 2 && index == 6)
        {
            // A helper may resolve after the following boss action; exact adjacency is not required.
            let delay = [400, 600, 1400].get(peer).copied().expect("peer delay");
            events.push(json!({"timestamp":at+delay,"type":"cast","sourceID":11,"abilityGameID":200,"fight":fight}));
            if scenario == "twice" {
                events.push(json!({"timestamp":at+2500,"type":"cast","sourceID":11,"abilityGameID":200,"fight":fight}));
            }
        }
    }
    events.push(json!({"timestamp":18000,"type":"death","targetID":10,"fight":fight}));
    if scenario == "ambient" {
        events.push(
            json!({"timestamp":1200,"type":"cast","sourceID":11,"abilityGameID":200,"fight":fight}),
        );
    }
    if scenario == "overlapping" {
        actors.push(
            json!({"id":20,"gameID":99003,"name":"Other Boss","type":"NPC","subType":"Boss"}),
        );
        enemies.push(json!({"id":20,"gameID":99003}));
        events.extend([
            json!({"timestamp":1500,"type":"cast","sourceID":20,"abilityGameID":9,"fight":fight}),
            json!({"timestamp":19000,"type":"cast","sourceID":20,"abilityGameID":4,"fight":fight}),
        ]);
    }
    events.sort_by_key(|event| event.get("timestamp").and_then(Value::as_u64));
    let abilities = vec![
        (9, "Opening"),
        (1, "Boundary"),
        (2, "Mechanic"),
        (3, "Follow"),
        (4, "Closing"),
        (
            200,
            if scenario == "different-name" {
                "Effect"
            } else if scenario == "ambiguous-name" && peer == 0 {
                "Other label"
            } else {
                "Mechanic"
            },
        ),
    ]
    .into_iter()
    .map(|(id, name)| json!({"gameID":id,"name":name,"type":"1"}))
    .collect::<Vec<_>>();
    for (pointer, value) in [
        ("/report/code", code.clone().into()),
        ("/collection/reportCode", code.into()),
        ("/report/fights/0/id", fight.into()),
        ("/collection/fightID", fight.into()),
        ("/report/masterData/actors", actors.into()),
        ("/report/masterData/abilities", abilities.into()),
        ("/report/fights/0/enemyNPCs", enemies.into()),
        ("/collection/eventCount", events.len().into()),
        ("/events", events.into()),
    ] {
        *data.pointer_mut(pointer).expect("fixture field") = value;
    }
    let path = directory.join(format!("{peer}.json"));
    fs::write(&path, serde_json::to_vec(&data).expect("encode fixture")).expect("write fixture");
    path
}

#[rstest::rstest]
#[case("local", "dungeon", true)]
#[case("local", "alliance", true)]
#[case("anonymous", "dungeon", true)]
#[case("missing", "dungeon", true)]
#[case("shared-report", "dungeon", true)]
#[case("different-name", "dungeon", true)]
#[case("twice", "dungeon", true)]
#[case("ambiguous-name", "dungeon", false)]
#[case("ambient", "dungeon", false)]
#[case("overlapping", "dungeon", false)]
#[case("one-time", "dungeon", false)]
fn helper_effects_extend_predictions_without_becoming_runtime_sources(
    #[case] scenario: &str,
    #[case] mode: &str,
    #[case] associated: bool,
) {
    let directory = tempfile::tempdir().unwrap();
    let paths: Vec<_> = (0..3)
        .map(|peer| input(directory.path(), peer, scenario))
        .collect();
    let output = directory.path().join("timeline.kdl");
    cargo_bin_cmd!("btimeline")
        .arg("generate")
        .args(paths)
        .args(["--mode", mode, "-o"])
        .arg(&output)
        .assert()
        .success();
    let evidence: Value =
        serde_json::from_slice(&fs::read(output.with_extension("evidence.json")).unwrap()).unwrap();
    let model = evidence["loops"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["source"] == "Synthetic Boss")
        .unwrap();
    assert_eq!(model["status"], "rendered");
    let core = model["events"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| {
            row["activities"]
                .as_array()
                .unwrap()
                .iter()
                .any(|activity| activity["identifier"]["Ability"] == 2)
        })
        .unwrap();
    let helper = model["events"].as_array().unwrap().iter().find(|row| {
        row["activities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|activity| activity["identifier"]["Ability"] == 200)
    });
    assert_eq!(helper.is_some(), associated);
    assert_eq!(
        model["events"].as_array().unwrap().len(),
        if scenario == "twice" {
            6
        } else if associated {
            5
        } else {
            4
        }
    );
    assert_eq!(core["offset_ms"], 1000);
    assert_eq!(
        model["helper_errors"].as_array().unwrap().is_empty(),
        scenario != "ambiguous-name"
    );
    for trace in evidence["progression"]["traces"].as_array().unwrap() {
        assert!(
            trace["nodes"]
                .as_array()
                .unwrap()
                .iter()
                .any(|node| node["activity"]["identifier"]["Ability"] == 200
                    && node["unclassified"] == true)
        );
    }
    let kdl = fs::read_to_string(&output).unwrap();
    assert!(!kdl.contains("Effect NPC"));
    if associated {
        let helper = helper.unwrap();
        assert_eq!(helper["offset_ms"], 1600);
        assert_eq!(helper["sync"], false);
        assert_eq!(
            helper["observations"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|row| row["helper"] == true)
                .count(),
            if scenario == "missing" { 8 } else { 9 }
        );
        fs::remove_file(output.with_extension("evidence.json")).unwrap();
        let recording = directory.path().join("recording.json");
        fs::write(&recording,serde_json::to_vec(&json!({"version":1,"end_ms":15000,"observations":[{"at_ms":0,"type":"Ability","fields":{"id":"9","source":"Synthetic Boss"}}]})).unwrap()).unwrap();
        let result = cargo_bin_cmd!("btimeline")
            .arg("compare")
            .arg(&output)
            .arg(&recording)
            .arg("--json")
            .assert()
            .success();
        let result: Value = serde_json::from_slice(&result.get_output().stdout).unwrap();
        let rows = result["recordings"][recording.to_str().unwrap()]
            .as_array()
            .unwrap();
        let prefix = format!("{}:", helper["event"].as_str().unwrap());
        let times: Vec<_> = rows
            .iter()
            .filter(|row| {
                row["action"] == "event" && row["detail"].as_str().unwrap().starts_with(&prefix)
            })
            .map(|row| row["at_ms"].clone())
            .collect();
        assert_eq!(
            times,
            vec![json!(2600), json!(6600), json!(10600), json!(14600)]
        );
        assert!(!rows.iter().any(|row| {
            ["ambiguous", "unresolved", "violation", "skipped"]
                .contains(&row["action"].as_str().unwrap())
        }));
    }
}
