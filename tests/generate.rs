use std::fs;
use std::path::{Path, PathBuf};

use assert_cmd::cargo::cargo_bin_cmd;
use serde_json::{Value, json};

const INPUT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/pull.json");
const TEMPLATE: &str = r#"timeline 1 "Generated common schedule" {
    start "Ability" id="B528" source="Synthetic Boss"
    wipe "ActorControl" command="4000000F"
    event "opening" 0.0 name="A"
}
"#;

fn graph(directory: &Path) -> PathBuf {
    let base: Value =
        serde_json::from_slice(&fs::read(INPUT).expect("read fixture")).expect("parse fixture");
    let paths: Vec<_> = [[2000, 6050, 10000], [2500, 6551, 10501]]
        .iter()
        .enumerate()
        .map(|(index, times)| {
            let mut data = base.clone();
            let code = format!("generation{index}");
            *data.pointer_mut("/report/code").expect("report code") = code.clone().into();
            *data
                .pointer_mut("/collection/reportCode")
                .expect("collection code") = code.into();
            *data
                .pointer_mut("/report/masterData/abilities")
                .expect("abilities") = json!([
            {"gameID":46376,"name":"A","type":"1"},
            {"gameID":46377,"name":"B","type":"1"},
            {"gameID":46378,"name":"C","type":"1"}]);
            *data.pointer_mut("/events").expect("events") = times
                .iter()
                .zip([46376, 46377, 46378])
                .map(|(at, id)| {
                    json!({
                        "timestamp":at,"type":"cast","sourceID":10,"abilityGameID":id,"fight":1
                    })
                })
                .collect();
            let path = directory.join(format!("pull{index}.json"));
            fs::write(&path, serde_json::to_vec(&data).expect("serialize input"))
                .expect("write input");
            path
        })
        .collect();
    let output = directory.join("process.kdl");
    cargo_bin_cmd!("btimeline")
        .arg("mine")
        .args(paths)
        .arg("-o")
        .arg(&output)
        .assert()
        .success();
    output
}

#[test]
fn collected_logs_generate_a_standalone_schedule_that_replays_and_restarts() {
    let dir = tempfile::tempdir().unwrap();
    let graph = graph(dir.path());
    let template = dir.path().join("origin.kdl");
    let output = dir.path().join("timeline.kdl");
    fs::write(&template, TEMPLATE).unwrap();
    cargo_bin_cmd!("btimeline")
        .arg("generate")
        .arg(&graph)
        .arg("--template")
        .arg(&template)
        .arg("-o")
        .arg(&output)
        .assert()
        .success();
    let timeline = fs::read_to_string(&output).unwrap();
    insta::assert_snapshot!("generated_common_timeline", timeline);
    assert!(!timeline.contains("FFLogs."));
    let evidence: Value =
        serde_json::from_slice(&fs::read(output.with_extension("evidence.json")).unwrap()).unwrap();
    let events = evidence["events"].as_array().unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0]["median_offset_ms"], 4050);
    assert_eq!(events[0]["rounded_offset_ms"], 4100);
    assert_eq!(events[0]["rounding_delta_ms"], 50);
    assert_eq!(events[0]["samples"][1]["offset_ms"], 4051);
    let recording = dir.path().join("recording.json");
    fs::write(
        &recording,
        serde_json::to_vec(&json!({"version":1,"end_ms":22000,"observations":[
            {"at_ms":0,"type":"Ability","fields":{"id":"B528","source":"Synthetic Boss"}},
            {"at_ms":12000,"type":"ActorControl","fields":{"command":"4000000F"}},
            {"at_ms":13000,"type":"Ability","fields":{"id":"B528","source":"Synthetic Boss"}}
        ]}))
        .unwrap(),
    )
    .unwrap();
    // Execution uses the KDL alone; remove evidence before replay to guard that boundary.
    fs::remove_file(output.with_extension("evidence.json")).unwrap();
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
    assert_eq!(displayed[1]["at_ms"], 4100);
    assert_eq!(displayed[4]["at_ms"], 17100);
    assert_eq!(fs::read_to_string(template).unwrap(), TEMPLATE);
}

#[rstest::rstest]
#[case("FFLogs.cast", "A", "0.0")]
#[case("Ability", "Unknown", "0.0")]
#[case("Ability", "A", "1.0")]
fn invalid_origin_contracts_leave_no_outputs(
    #[case] kind: &str,
    #[case] name: &str,
    #[case] time: &str,
) {
    let dir = tempfile::tempdir().unwrap();
    let graph = graph(dir.path());
    let template = dir.path().join("origin.kdl");
    let output = dir.path().join("timeline.kdl");
    fs::write(&template, format!("timeline 1 \"Invalid\" {{\n start \"{kind}\" id=\"B528\"\n wipe \"ActorControl\" command=\"4000000F\"\n event \"opening\" {time} name=\"{name}\"\n}}\n")).unwrap();
    let result = cargo_bin_cmd!("btimeline")
        .arg("generate")
        .arg(&graph)
        .arg("--template")
        .arg(template)
        .arg("-o")
        .arg(&output)
        .assert()
        .failure();
    assert!(result.get_output().stdout.is_empty());
    assert!(!output.exists());
    assert!(!output.with_extension("evidence.json").exists());
}

#[test]
fn blocked_evidence_preserves_existing_files_and_rolls_back_new_timeline() {
    let dir = tempfile::tempdir().unwrap();
    let graph = graph(dir.path());
    let original_graph = fs::read(&graph).unwrap();
    let template = dir.path().join("origin.kdl");
    let output = dir.path().join("timeline.kdl");
    let report = output.with_extension("evidence.json");
    fs::write(&template, TEMPLATE).unwrap();
    fs::write(&report, "previous evidence").unwrap();
    cargo_bin_cmd!("btimeline")
        .arg("generate")
        .arg(&graph)
        .arg("--template")
        .arg(template)
        .arg("-o")
        .arg(&output)
        .assert()
        .failure();
    assert!(!output.exists());
    assert_eq!(fs::read_to_string(report).unwrap(), "previous evidence");
    assert_eq!(fs::read(graph).unwrap(), original_graph);
}

#[test]
fn events_before_the_authored_origin_are_reported_instead_of_moved() {
    let dir = tempfile::tempdir().unwrap();
    let graph = graph(dir.path());
    let template = dir.path().join("origin.kdl");
    let output = dir.path().join("timeline.kdl");
    fs::write(
        &template,
        TEMPLATE
            .replace("B528", "B529")
            .replace("name=\"A\"", "name=\"B\""),
    )
    .unwrap();
    cargo_bin_cmd!("btimeline")
        .arg("generate")
        .arg(&graph)
        .arg("--template")
        .arg(template)
        .arg("-o")
        .arg(&output)
        .assert()
        .success();
    let evidence: Value =
        serde_json::from_slice(&fs::read(output.with_extension("evidence.json")).unwrap()).unwrap();
    assert_eq!(evidence["events"].as_array().unwrap().len(), 1);
    assert_eq!(evidence["events"][0]["median_offset_ms"], 3950);
    assert_eq!(
        evidence["excluded"][0]["reason"],
        "before authored origin in a completed trace"
    );
    assert_eq!(evidence["excluded"][0]["samples"][0]["offset_ms"], -4050);
}

#[rstest::rstest]
#[case(true)]
#[case(false)]
fn invalid_common_samples_do_not_generate_outputs(#[case] duplicate: bool) {
    let dir = tempfile::tempdir().unwrap();
    let graph = graph(dir.path());
    let template = dir.path().join("origin.kdl");
    let output = dir.path().join("timeline.kdl");
    fs::write(&template, TEMPLATE).unwrap();
    let mut document: kdl::KdlDocument = fs::read_to_string(&graph).unwrap().parse().unwrap();
    let common = document.nodes_mut()[0]
        .ensure_children()
        .nodes_mut()
        .iter_mut()
        .find(|node| node.name().value() == "common-progress")
        .unwrap();
    let anchor = common
        .ensure_children()
        .nodes_mut()
        .iter_mut()
        .find(|node| node.name().value() == "anchor")
        .unwrap();
    let samples = anchor.ensure_children().nodes_mut();
    if duplicate {
        samples.push(samples[0].clone());
    } else {
        samples.pop();
    }
    fs::write(&graph, document.to_string()).unwrap();
    cargo_bin_cmd!("btimeline")
        .arg("generate")
        .arg(graph)
        .arg("--template")
        .arg(template)
        .arg("-o")
        .arg(&output)
        .assert()
        .failure();
    assert!(!output.exists());
    assert!(!output.with_extension("evidence.json").exists());
}
