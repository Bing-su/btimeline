use std::fs;
use std::path::{Path, PathBuf};

use assert_cmd::cargo::cargo_bin_cmd;
use serde_json::{Value, json};

fn fixture(directory: &Path, code: &str, events: Vec<Value>) -> PathBuf {
    let mut data: Value =
        serde_json::from_str(include_str!("fixtures/pull.json")).expect("fixture JSON");
    data.pointer_mut("/report/masterData/actors")
        .and_then(Value::as_array_mut)
        .expect("actors")
        .push(json!({"id":11,"gameID":99002,"name":"Synthetic Boss","type":"NPC","subType":"NPC"}));
    data.pointer_mut("/report/fights/0/enemyNPCs")
        .and_then(Value::as_array_mut)
        .expect("enemy NPCs")
        .push(json!({"id":11,"gameID":99002}));
    for (pointer, value) in [
        ("/report/code", json!(code)),
        ("/collection/reportCode", json!(code)),
        (
            "/report/masterData/abilities",
            json!([
                {"gameID":100,"name":"Opening","type":"1"},
                {"gameID":200,"name":"Repeated","type":"1"},
                {"gameID":300,"name":"Late helper","type":"1"}
            ]),
        ),
        ("/collection/eventCount", json!(events.len())),
        ("/events", json!(events)),
    ] {
        *data.pointer_mut(pointer).expect("fixture field") = value;
    }
    let path = directory.join(format!("{code}.json"));
    fs::write(&path, serde_json::to_vec(&data).expect("encode fixture")).expect("write fixture");
    path
}

#[test]
fn varying_repeated_choices_do_not_hide_shared_repeated_casts() {
    let dir = tempfile::tempdir().unwrap();
    let paths: Vec<_> = (0..3).map(|peer| {
        let ids = if peer == 0 {
            vec![100,200,400,100,200,300,400,400,300]
        } else {
            vec![100,200,400,400,300,100,100,200,200,300]
        };
        let mut events:Vec<_>=ids.iter().enumerate().map(|(index,id)|json!({"timestamp":2000+index*1000,"type":"cast","sourceID":10,"abilityGameID":id,"fight":1})).collect();
        events.push(json!({"timestamp":18000,"type":"death","targetID":10,"fight":1}));
        let path=fixture(dir.path(),&format!("choices{peer}"),events);
        let mut data:Value=serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        data["report"]["masterData"]["abilities"].as_array_mut().unwrap().push(json!({"gameID":400,"name":"Alternative","type":"1"}));
        fs::write(&path,serde_json::to_vec(&data).unwrap()).unwrap();
        path
    }).collect();
    let output = dir.path().join("timeline.kdl");
    cargo_bin_cmd!("btimeline")
        .arg("generate")
        .args(paths)
        .args(["--mode", "dungeon", "-o"])
        .arg(&output)
        .assert()
        .success();
    let evidence: Value =
        serde_json::from_slice(&fs::read(output.with_extension("evidence.json")).unwrap()).unwrap();
    let casts: Vec<_> = evidence["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["activity"]["identifier"]["Ability"] == 300)
        .collect();
    assert_eq!(
        casts.len(),
        2,
        "Shared repeated casts must survive unrelated choice/count differences"
    );
    for cast in casts {
        assert_eq!(cast["samples"].as_array().unwrap().len(), 3);
        assert!(
            cast["alignment"]["sync"]
                .as_str()
                .unwrap()
                .contains("omitted")
        );
    }
}

#[rstest::rstest]
#[case("dungeon", "damage", true)]
#[case("alliance", "damage", true)]
#[case("dungeon", "death", true)]
#[case("dungeon", "entry-phase", true)]
#[case("dungeon", "missing", false)]
#[case("dungeon", "unknown-instance", false)]
#[case("dungeon", "helper-only", false)]
#[case("dungeon", "two-completions", false)]
#[case("dungeon", "different-entry", false)]
fn completed_attempts_keep_raw_evidence_without_requiring_unique_abilities(
    #[case] mode: &str,
    #[case] completion: &str,
    #[case] succeeds: bool,
) {
    let dir = tempfile::tempdir().unwrap();
    let mut events:Vec<_>=[(2000,100,1),(4000,200,1),(10000,100,2),(12000,200,2),(14000,100,2),(16000,200,2)]
        .iter().map(|(at,id,instance)|json!({"timestamp":at,"type":"cast","sourceID":10,"sourceInstance":instance,"abilityGameID":id,"fight":1})).collect();
    let mut death = json!({"timestamp":18000,"type":"damage","targetID":10,"targetInstance":2,"targetResources":{"hitPoints":0,"maxHitPoints":100},"fight":1});
    match completion {
        "death" => death["type"] = "death".into(),
        "unknown-instance" => {
            death.as_object_mut().unwrap().remove("targetInstance");
        }
        "helper-only" => death["targetID"] = 11.into(),
        "two-completions" => events.push(
            json!({"timestamp":5000,"type":"death","targetID":10,"targetInstance":1,"fight":1}),
        ),
        _ => {}
    }
    if completion != "missing" {
        events.push(death);
    }
    // Same-name helper packets after the observed defeat remain raw evidence, not a second boss attempt.
    events
        .push(json!({"timestamp":19000,"type":"cast","sourceID":11,"abilityGameID":300,"fight":1}));
    let paths: Vec<_> = (0..3)
        .map(|peer| {
            let mut records = events.clone();
            if completion == "different-entry" && peer == 2 {
                records
                    .iter_mut()
                    .find(|event| event["timestamp"] == 10000)
                    .unwrap()["abilityGameID"] = 200.into();
            }
            fixture(dir.path(), &format!("attempt{peer}"), records)
        })
        .collect();
    let output = dir.path().join("timeline.kdl");
    if completion == "entry-phase" {
        for path in &paths {
            let mut data: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
            data["report"]["fights"][0]["phaseTransitions"] = json!([{"id":2,"startTime":10000}]);
            fs::write(path, serde_json::to_vec(&data).unwrap()).unwrap();
        }
    }
    let result = cargo_bin_cmd!("btimeline")
        .arg("generate")
        .args(paths)
        .args(["--mode", mode, "-o"])
        .arg(&output)
        .assert();
    if !succeeds {
        result.failure();
        assert!(!output.exists());
        return;
    }
    result.success();
    let evidence: Value =
        serde_json::from_slice(&fs::read(output.with_extension("evidence.json")).unwrap()).unwrap();
    let rows = evidence["events"].as_array().unwrap();
    assert_eq!(rows.len(), 4);
    assert!(
        rows.iter()
            .all(|row| row["alignment"]["method"] == "boss-entry-lcs-v1")
    );
    for row in rows {
        assert!(
            row["samples"]
                .as_array()
                .unwrap()
                .iter()
                .all(|sample| sample["at_ms"].as_i64().unwrap() >= 9000)
        );
    }
    assert!(
        evidence["progression"]["traces"]
            .as_array()
            .unwrap()
            .iter()
            .all(|trace| trace["nodes"].as_array().unwrap().len() == 7)
    );
    assert!(
        evidence["alignment_diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["method"] == "boss-attempt-scope-v1"
                && row["attempts"].as_array().unwrap().len() == 2
                && row["selected"] == json!([9000, 17000]))
    );
    let validation = evidence["alignment_diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["method"] == "automatic-validation-v1")
        .unwrap();
    let checked = &validation["checks"][0]["scopes"][0]["result"];
    assert_eq!(checked["anchor_at_ms"], 9000);
    assert!(checked["events"].as_array().unwrap().iter().any(
        |row| row["event"]["activity"]["identifier"]["Ability"] == 100
            && row["event"]["reference_at_ms"] == 13000
            && row["at_ms"] == 13000
    ));
}

#[test]
fn boss_defeat_censors_a_tail_even_while_the_dungeon_recording_continues() {
    let dir = tempfile::tempdir().unwrap();
    let paths:Vec<_>=(0..3).map(|peer| {
        let mut events=vec![json!({"timestamp":2000,"type":"cast","sourceID":10,"abilityGameID":100,"fight":1}),
            json!({"timestamp":6000,"type":"cast","sourceID":10,"abilityGameID":200,"fight":1})];
        let end=if peer==2 {8000} else {12000};
        if peer!=2 {events.push(json!({"timestamp":10000,"type":"cast","sourceID":10,"abilityGameID":200,"fight":1}));}
        events.push(json!({"timestamp":end,"type":"damage","targetID":10,"targetResources":{"hitPoints":0,"maxHitPoints":100},"fight":1}));
        fixture(dir.path(),&format!("tail{peer}"),events)
    }).collect();
    let output = dir.path().join("timeline.kdl");
    cargo_bin_cmd!("btimeline")
        .arg("generate")
        .args(paths)
        .args(["--mode", "dungeon", "-o"])
        .arg(&output)
        .assert()
        .success();
    let evidence: Value =
        serde_json::from_slice(&fs::read(output.with_extension("evidence.json")).unwrap()).unwrap();
    let tail = evidence["events"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["event"].as_str().unwrap().ends_with("-02"))
        .unwrap();
    assert_eq!(tail["samples"].as_array().unwrap().len(), 2);
    let validation = evidence["alignment_diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["method"] == "automatic-validation-v1")
        .unwrap();
    assert!(
        validation["checks"][0]["scopes"][0]["result"]["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(
                |event| event["event"]["activity"]["identifier"]["Ability"] == 200
                    && event["status"] == "unobserved"
            )
    );
}

#[test]
fn virtual_helpers_marked_boss_do_not_define_body_completion() {
    let dir = tempfile::tempdir().unwrap();
    let paths:Vec<_>=(0..3).map(|peer| {
        let path=fixture(dir.path(),&format!("virtual{peer}"),vec![
            json!({"timestamp":2000,"type":"cast","sourceID":10,"abilityGameID":100,"fight":1}),
            json!({"timestamp":4000,"type":"cast","sourceID":11,"abilityGameID":200,"fight":1}),
            json!({"timestamp":6000,"type":"cast","sourceID":10,"abilityGameID":300,"fight":1}),
            json!({"timestamp":8000,"type":"damage","targetID":10,"targetResources":{"hitPoints":0},"fight":1}),
            json!({"timestamp":10000,"type":"cast","sourceID":11,"abilityGameID":200,"fight":1}),
        ]);
        let mut data:Value=serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        data["report"]["masterData"]["actors"][1]["gameID"]=2_000_011.into();
        data["report"]["masterData"]["actors"][1]["subType"]="Boss".into();
        data["report"]["fights"][0]["enemyNPCs"][1]["gameID"]=2_000_011.into();
        fs::write(&path,serde_json::to_vec(&data).unwrap()).unwrap();
        path
    }).collect();
    let output = dir.path().join("timeline.kdl");
    cargo_bin_cmd!("btimeline")
        .arg("generate")
        .args(paths)
        .args(["--mode", "dungeon", "-o"])
        .arg(&output)
        .assert()
        .success();
    let evidence: Value =
        serde_json::from_slice(&fs::read(output.with_extension("evidence.json")).unwrap()).unwrap();
    assert_eq!(evidence["events"].as_array().unwrap().len(), 3);
    assert!(
        evidence["progression"]["traces"]
            .as_array()
            .unwrap()
            .iter()
            .all(|trace| trace["nodes"].as_array().unwrap().len() == 4)
    );
    assert!(
        evidence["alignment_diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|row| row["method"] == "boss-attempt-scope-v1")
            .all(|row| row["attempts"].as_array().unwrap().len() == 1
                && row["selected"] == json!([1000, 7000]))
    );
}
