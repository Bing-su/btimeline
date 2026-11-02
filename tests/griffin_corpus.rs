use std::fs;

use assert_cmd::cargo::cargo_bin_cmd;
use serde_json::{Value, json};

#[test]
fn previously_evaluated_griffin_casts_keep_their_advance_forecasts() {
    let report: Value = serde_json::from_str(include_str!(
        "../timelines/mistwake-griffin.holdout-result.json"
    ))
    .expect("saved evaluation");
    let plan_source = include_str!("fixtures/authored/griffin-holdout-plan.json");
    let plan: Value = serde_json::from_str(plan_source).expect("frozen criteria");
    let plan_hash: String = ring::digest::digest(&ring::digest::SHA256, plan_source.as_bytes())
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    assert_eq!(report["plan_sha256"], plan_hash);
    assert_eq!(report["timeline_sha256"], plan["timeline_sha256"]);
    let directory = tempfile::tempdir().expect("temporary directory");
    let timeline = directory.path().join("griffin.kdl");
    fs::write(&timeline, include_str!("../timelines/mistwake-griffin.kdl"))
        .expect("standalone timeline");
    let recording = directory.path().join("recording.json");
    let lead = plan["forecast_lead_ms"].as_i64().expect("forecast lead");
    let tolerance = plan["tolerance_ms"].as_u64().expect("tolerance");
    let mut traces = 0;
    let mut queries = 0;
    for trace in report["traces"].as_array().expect("traces") {
        if trace["status"] != "evaluated" {
            continue;
        }
        traces += 1;
        let targets = trace["targets"].as_array().expect("observed targets");
        // Preserve future observations in the recording to exercise cutoffs; no raw logs or evidence reach the executor.
        let observations: Vec<_> = targets
            .iter()
            .map(|target| {
                json!({"at_ms":target["at_ms"],"type":"Ability","fields":{
                    "id":target["id"],
                    "source":if target["id"] == "B0F4" {"Shock Storm"} else {"Thundergust Griffin"}
                }})
            })
            .collect();
        fs::write(
            &recording,
            serde_json::to_vec(
                &json!({"version":1,"end_ms":trace["end_ms"],"observations":observations}),
            )
            .expect("encode recording"),
        )
        .expect("write recording");
        let entry = trace["entry_ms"].as_i64().expect("entry");
        for target in targets {
            let at = target["at_ms"].as_i64().expect("observed time");
            if at - entry < lead {
                continue;
            }
            queries += 1;
            let output = cargo_bin_cmd!("btimeline")
                .current_dir(directory.path())
                .arg("forecast")
                .arg(&timeline)
                .arg(&recording)
                .arg("--at-ms")
                .arg((at - lead).to_string())
                .arg("--horizon-ms")
                .arg(plan["horizon_ms"].as_i64().expect("horizon").to_string())
                .assert()
                .success();
            let forecast: Value =
                serde_json::from_slice(&output.get_output().stdout).expect("forecast JSON");
            assert_eq!(forecast["status"], "active");
            assert_eq!(forecast["diagnostics"], json!([]));
            assert_eq!(forecast["projection_diagnostics"], json!([]));
            let observed = targets
                .iter()
                .filter(|item| item["at_ms"].as_i64().expect("observed time") <= at - lead)
                .count();
            assert_eq!(forecast["observations_used"], json!(observed));
            let name = &plan["targets"][target["id"].as_str().expect("ability ID")];
            assert!(name.is_string(), "Unknown target: {target}");
            assert!(
                forecast["events"]
                    .as_array()
                    .expect("events")
                    .iter()
                    .any(|event| {
                        event["name"] == *name
                            && event["at_ms"].as_i64().expect("forecast time").abs_diff(at)
                                <= tolerance
                    }),
                "{} raw #{} at {at}: {forecast}",
                trace["trace"],
                target["raw_index"]
            );
        }
    }
    // Keep corpus shrinkage from silently weakening this offline regression check.
    assert_eq!((traces, queries), (4, 72));
}
