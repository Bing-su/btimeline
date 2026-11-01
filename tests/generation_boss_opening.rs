use std::fs;
use std::path::Path;

use anyhow::Context;
use assert_cmd::cargo::cargo_bin_cmd;
use serde_json::{Value, json};

fn input(directory: &Path, peer: i64) -> anyhow::Result<std::path::PathBuf> {
    let mut data: Value = serde_json::from_str(include_str!("fixtures/pull.json"))?;
    let code = format!("bossopening{peer}");
    data.pointer_mut("/report/masterData/actors")
        .and_then(Value::as_array_mut)
        .context("fixture actors")?
        .push(json!({"id":20,"gameID":99002,"name":"Second Boss","type":"NPC","subType":"Boss"}));
    data.pointer_mut("/report/fights/0/enemyNPCs")
        .and_then(Value::as_array_mut)
        .context("fixture NPCs")?
        .push(json!({"id":20,"gameID":99002}));
    data.pointer_mut("/report/fights/0")
        .and_then(Value::as_object_mut)
        .context("fixture fight")?
        .insert(
            "phaseTransitions".into(),
            json!([{"id":2,"startTime":10000+peer*100}]),
        );
    let abilities = json!([
        {"gameID":100,"name":"Repeated opening","type":"1"},
        {"gameID":200,"name":"Unique continuation","type":"1"}
    ]);
    let events = [(2000,100,10),(4000,100,10),(6000,200,10),(12000,100,20),(14000,100,20),(16000,200,20)]
        .iter().map(|(at,id,source)|json!({"timestamp":at+peer*100,"type":"cast","sourceID":source,"abilityGameID":id,"fight":1})).collect();
    for (pointer, value) in [
        ("/report/code", json!(code)),
        ("/collection/reportCode", json!(code)),
        ("/report/masterData/abilities", abilities),
        ("/events", events),
        ("/collection/eventCount", json!(6)),
    ] {
        *data.pointer_mut(pointer).context("fixture field")? = value;
    }
    let path = directory.join(format!("{peer}.json"));
    fs::write(&path, serde_json::to_vec(&data)?)?;
    Ok(path)
}

#[rstest::rstest]
#[case("dungeon")]
#[case("alliance")]
fn repeated_openings_enter_once_and_preserve_each_boss_clock(#[case] mode: &str) {
    let dir = tempfile::tempdir().unwrap();
    let inputs = [input(dir.path(), 0).unwrap(), input(dir.path(), 1).unwrap()];
    let output = dir.path().join("bosses.kdl");
    cargo_bin_cmd!("btimeline")
        .arg("generate")
        .args(inputs)
        .args(["--mode", mode, "-o"])
        .arg(&output)
        .assert()
        .success();
    let evidence: Value =
        serde_json::from_slice(&fs::read(output.with_extension("evidence.json")).unwrap()).unwrap();
    assert_eq!(evidence["events"].as_array().unwrap().len(), 6);
    for origin in evidence["origins"].as_array().unwrap() {
        assert_eq!(origin["activities"][0]["identifier"]["Ability"], 100);
    }
    let doc: kdl::KdlDocument = fs::read_to_string(&output).unwrap().parse().unwrap();
    let phases: Vec<_> = doc.nodes()[0]
        .children()
        .unwrap()
        .nodes()
        .iter()
        .filter(|node| node.name().value() == "phase")
        .collect();
    for phase in phases {
        let repeated: Vec<_> = phase
            .children()
            .unwrap()
            .nodes()
            .iter()
            .filter(|node| {
                node.name().value() == "event"
                    && node.get("name").and_then(kdl::KdlValue::as_string)
                        == Some("Repeated opening")
            })
            .collect();
        assert_eq!(repeated.len(), 2);
        assert!(
            repeated.iter().all(|node| node.children().is_none()),
            "phase entry must not grant repeated-ID sync permission"
        );
    }
    let observations: Vec<_> = [
        (1000, "64", "Synthetic Boss"),
        (3000, "64", "Synthetic Boss"),
        (5000, "C8", "Synthetic Boss"),
        (11000, "64", "Second Boss"),
        (13000, "64", "Second Boss"),
        (15000, "C8", "Second Boss"),
    ]
    .iter()
    .map(|(at, id, source)| json!({"at_ms":at,"type":"Ability","fields":{"id":id,"source":source}}))
    .collect();
    let recording = dir.path().join("recording.json");
    fs::write(
        &recording,
        serde_json::to_vec(&json!({"version":1,"end_ms":19000,"observations":observations}))
            .unwrap(),
    )
    .unwrap();
    // Replay must depend on KDL alone, including both repeated-ID entries and the intervening travel.
    fs::remove_file(output.with_extension("evidence.json")).unwrap();
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
    assert_eq!(
        records
            .iter()
            .filter(|row| row["action"] == "phase")
            .count(),
        2
    );
    assert_eq!(
        records
            .iter()
            .filter(|row| row["action"] == "start")
            .count(),
        1
    );
    let displayed: Vec<_> = records
        .iter()
        .filter(|row| row["action"] == "event")
        .map(|row| row["at_ms"].as_i64().unwrap())
        .collect();
    assert_eq!(displayed, vec![1000, 3000, 5000, 11000, 13000, 15000]);
    assert!(!records.iter().any(|row| {
        ["ambiguous", "unresolved", "violation", "skipped"]
            .contains(&row["action"].as_str().unwrap())
    }));
}
