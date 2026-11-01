use std::fs;
use std::path::{Path, PathBuf};

use assert_cmd::cargo::cargo_bin_cmd;
use proptest::prelude::*;
use serde_json::{Value, json};

#[test]
fn local_reference_ordinals_do_not_collide_across_intervals() {
    let dir = tempfile::tempdir().unwrap();
    let paths: Vec<_> = (0..3).map(|peer| {
        let path = input(dir.path(),peer,"split",100,50);
        let mut data:Value=serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        data["report"]["masterData"]["abilities"].as_array_mut().unwrap().push(json!({"gameID":104,"name":"Extra","type":"1"}));
        let mut records=vec![(2000,100,1),(4000,101,1),(4000+if peer==1 {50} else {0},101,2),(6000,102,1),(8000,101,1),(10000,103,1),(12000,101,1)];
        if peer==1 {records.extend([(3000,104,1),(3500,104,1)]);} else {records.push((14000,101,1));}
        data["events"]=records.iter().map(|(at,id,instance)|json!({"timestamp":at,"type":"cast","sourceID":10,"sourceInstance":instance,"abilityGameID":id,"fight":1})).collect();
        data["collection"]["eventCount"]=records.len().into();
        fs::write(&path,serde_json::to_vec(&data).unwrap()).unwrap();
        path
    }).collect();
    let output = dir.path().join("timeline.kdl");
    let evidence = generate(&paths, &output);
    let rows: Vec<_> = evidence["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["activity"]["identifier"]["Ability"] == 101)
        .collect();
    assert_eq!(rows.len(), 3);
    let bounded = rows
        .iter()
        .find(|row| row["alignment"]["bounds"]["left"]["identifier"]["Ability"] == 102)
        .unwrap();
    let tail = rows
        .iter()
        .find(|row| row["alignment"]["method"] == "censored-tail-v1")
        .unwrap();
    assert_eq!(bounded["alignment"]["reference"], "fragments1/1");
    assert_eq!(tail["alignment"]["reference"], "fragments0/1");
    assert_ne!(bounded["event"], tail["event"]);
    assert!(bounded["event"].as_str().unwrap().contains("after-66"));
    assert!(tail["event"].as_str().unwrap().contains("after-67"));
    let reverse = dir.path().join("reverse.kdl");
    generate(&paths.into_iter().rev().collect::<Vec<_>>(), &reverse);
    assert_eq!(
        fs::read_to_string(output).unwrap(),
        fs::read_to_string(reverse).unwrap()
    );
}

fn input(dir: &Path, peer: usize, scenario: &str, base: i64, gap: i64) -> PathBuf {
    let mut data: Value =
        serde_json::from_str(include_str!("fixtures/pull.json")).expect("fixture");
    let split = peer == 0 || scenario == "all-split" || (scenario == "heldout-only" && peer < 2);
    let mut records = vec![
        (2000, base, 1),
        (4000, base + 1, 1),
        (
            4000 + if split { gap } else { 0 },
            base + 1,
            if scenario == "same-entity" { 1 } else { 2 },
        ),
        (8000, base + 1, 1),
        (8000, base + 1, 2),
        (12000, base + 2, 1),
    ];
    if scenario == "long-chain" && peer == 0 {
        records.push((4000 + gap * 2, base + 1, 3));
    }
    if scenario == "different-member" && peer == 0 {
        records.push((4000, base + 3, 1));
    }
    let code = format!("fragments{peer}");
    for (pointer,value) in [
        ("/report/code",code.clone().into()),("/collection/reportCode",code.into()),
        ("/report/masterData/abilities",(0..4).map(|id|json!({"gameID":base+id,"name":format!("Spell{id}"),"type":"1"})).collect()),
        ("/events",records.iter().map(|(at,id,instance)|json!({"timestamp":at,"type":"cast","sourceID":10,"sourceInstance":instance,"abilityGameID":id,"fight":1})).collect()),
        ("/collection/eventCount",records.len().into())
    ] {
        *data.pointer_mut(pointer).expect("fixture field") = value;
    }
    let path = dir.join(format!("{peer}.json"));
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
    serde_json::from_slice(
        &fs::read(output.with_extension("evidence.json")).expect("read evidence"),
    )
    .expect("evidence")
}

#[rstest::rstest]
#[case("split", 1, 2)]
#[case("split", 2, 2)]
#[case("split", 100, 2)]
#[case("same-entity", 2, 0)]
#[case("all-split", 2, 1)]
#[case("long-chain", 80, 0)]
#[case("different-member", 2, 0)]
fn fragmented_instances_need_a_forced_unfragmented_peer(
    #[case] scenario: &str,
    #[case] gap: i64,
    #[case] expected: usize,
) {
    let dir = tempfile::tempdir().unwrap();
    let paths: Vec<_> = (0..2)
        .map(|peer| input(dir.path(), peer, scenario, 100, gap))
        .collect();
    let output = dir.path().join("timeline.kdl");
    let e = generate(&paths, &output);
    let events: Vec<_> = e["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|event| event["activity"]["identifier"]["Ability"] == 101)
        .collect();
    assert_eq!(events.len(), expected, "{scenario}");
    if scenario == "split" {
        let event = events
            .iter()
            .find(|event| !event["alignment"]["fragments"].is_null())
            .unwrap();
        let fragments = &event["alignment"]["fragments"]["fragments0/1"];
        assert_eq!(fragments["at_ms"], json!([3000, 3000 + gap]));
        assert_eq!(fragments["instances"], 2);
        let raw = e["analysis"]["inputs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|input| input["trace"] == "fragments0/1")
            .unwrap();
        for batch in fragments["batches"].as_array().unwrap() {
            let batch = batch.as_u64().unwrap() as usize;
            assert_eq!(
                raw["observations"][batch]["signals"]
                    .as_array()
                    .unwrap()
                    .len(),
                1
            );
        }
        let diagnostic = e["alignment_diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["trace"] == "fragments0/1" && row.get("unrepresented").is_some())
            .unwrap();
        assert!(
            diagnostic["unrepresented"]
                .as_array()
                .unwrap()
                .iter()
                .all(|row| row["activity"]["identifier"]["Ability"] != 101)
        );
        // Fragment attribution remains optional at execution: the standalone KDL only shows one wave.
        let doc: kdl::KdlDocument = fs::read_to_string(&output).unwrap().parse().unwrap();
        let wave_events: Vec<_> = doc.nodes()[0]
            .children()
            .unwrap()
            .nodes()
            .iter()
            .filter(|node| node.get("name").and_then(kdl::KdlValue::as_string) == Some("Spell1"))
            .collect();
        assert_eq!(wave_events.len(), 2);
        assert!(wave_events.iter().all(|event| event.children().is_none()));
        if gap == 2 {
            insta::assert_snapshot!(
                "generated_fragmented_wave",
                fs::read_to_string(&output).unwrap()
            );
        }
    }
    let reverse = dir.path().join("reverse.kdl");
    generate(&paths.into_iter().rev().collect::<Vec<_>>(), &reverse);
    assert_eq!(fs::read(output).unwrap(), fs::read(reverse).unwrap());
}

#[test]
fn a_heldout_single_wave_cannot_normalize_the_training_fragments() {
    let dir = tempfile::tempdir().unwrap();
    let paths: Vec<_> = (0..3)
        .map(|peer| input(dir.path(), peer, "heldout-only", 200, 2))
        .collect();
    let e = generate(&paths, &dir.path().join("timeline.kdl"));
    let validation = e["alignment_diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["method"] == "automatic-validation-v1")
        .unwrap();
    assert_eq!(validation["split"]["held_out"], "fragments2/1");
    assert_eq!(
        validation["model_events"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|event| event["activity"]["identifier"]["Ability"] == 201)
            .count(),
        1
    );
    assert_eq!(
        e["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|event| event["activity"]["identifier"]["Ability"] == 201)
            .count(),
        2
    );
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(16))]
    #[test]
    fn disjoint_fragment_recovery_does_not_depend_on_ability_id(base in 1_i64..60000,gap in 1_i64..101) {
        let dir = tempfile::tempdir().unwrap();
        let paths:Vec<_> = (0..2).map(|peer|input(dir.path(),peer,"split",base,gap)).collect();
        let e = generate(&paths,&dir.path().join("timeline.kdl"));
        let events = e["events"].as_array().unwrap();
        prop_assert_eq!(events.iter().filter(|event|event["activity"]["identifier"]["Ability"]==base+1).count(),2);
    }
}
