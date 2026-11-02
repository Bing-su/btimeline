use std::fs;
use std::path::Path;

use assert_cmd::cargo::cargo_bin_cmd;
use ring::digest::{SHA256, digest};
use serde_json::{Value, json};

fn hash(bytes: &[u8]) -> String {
    digest(&SHA256, bytes)
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
fn setup(directory: &Path) -> Value {
    let timeline=b"timeline 1 \"Test\" { start \"Ability\" id=\"B528\"; phase \"boss\" { enter \"Ability\" id=\"B528\"; event \"first\" 0.0 name=\"Hit\"; event \"second\" 4.0 name=\"Hit\"; event \"third\" 8.0 name=\"Hit\"; }; }";
    let raw = include_bytes!("fixtures/pull.json");
    fs::write(directory.join("timeline.kdl"), timeline).expect("valid test fixture");
    fs::write(directory.join("input.json"), raw).expect("valid test fixture");
    fs::write(directory.join("profile.json"),serde_json::to_vec(&json!({"version":1,"observations":"all_casts","segments":[{"phase":"boss","source":"Synthetic Boss","entry":["B528"],"completion":"fight_end","targets":{"B528":{"name":"Hit"}}}],"not_checked":[]})).expect("valid test fixture")).expect("valid test fixture");
    json!({"timeline":"timeline.kdl","timeline_sha256":hash(timeline),"role":"test","forecast_lead_ms":5000,"horizon_ms":6500,"tolerance_ms":1500,"inputs":[{"input":"input.json","sha256":hash(raw)}]})
}
fn save(directory: &Path, manifest: &Value) {
    fs::write(
        directory.join("manifest.json"),
        serde_json::to_vec(manifest).expect("valid test fixture"),
    )
    .expect("valid test fixture");
}
fn command(directory: &Path) -> assert_cmd::Command {
    let mut command = cargo_bin_cmd!("btimeline");
    command.current_dir(directory).args([
        "evaluate",
        "--profile",
        "profile.json",
        "manifest.json",
        "-o",
        "report.json",
    ]);
    command
}

#[test]
fn native_evaluation_matches_forecasts_binds_profile_and_preserves_reports() {
    let directory = tempfile::tempdir().expect("valid test fixture");
    let manifest = setup(directory.path());
    save(directory.path(), &manifest);
    command(directory.path()).assert().success();
    let original = fs::read(directory.path().join("report.json")).expect("valid test fixture");
    let report: Value = serde_json::from_slice(&original).expect("valid test fixture");
    assert_eq!(
        report["summary"],
        json!({"selected":1,"excluded":0,"eligible":1,"passed_traces":1,"passed":true})
    );
    assert_eq!(
        report["profile_sha256"],
        hash(&fs::read(directory.path().join("profile.json")).unwrap())
    );
    assert_eq!(
        report["traces"][0]["bosses"][0]["targets"][2]["forecast"]["error_ms"],
        0
    );
    command(directory.path()).assert().failure();
    assert_eq!(
        fs::read(directory.path().join("report.json")).unwrap(),
        original
    );
}

#[test]
fn input_failures_are_reported_without_excluding_execution_or_validation_errors() {
    let directory = tempfile::tempdir().expect("valid test fixture");
    let mut manifest = setup(directory.path());
    fs::write(directory.path().join("bad.json"), "{}").expect("valid test fixture");
    let original = manifest["inputs"][0].clone();
    manifest["inputs"] = json!([original,{"input":"bad.json","sha256":hash(b"{}")},{"input":"missing.json","sha256":"unused"},original]);
    save(directory.path(), &manifest);
    command(directory.path()).assert().failure();
    let report: Value = serde_json::from_slice(
        &fs::read(directory.path().join("report.json")).expect("valid test fixture"),
    )
    .expect("valid test fixture");
    assert_eq!(
        report["summary"],
        json!({"selected":4,"excluded":2,"eligible":2,"passed_traces":1,"passed":false})
    );
    assert_eq!(report["traces"][1]["status"], "error");
    assert_eq!(report["traces"][1]["stage"], "input-inspection");
    assert_eq!(
        report["traces"][3]["error"],
        "InputExclusion: Duplicate mechanic sequence"
    );
}

#[test]
fn changed_candidate_and_invalid_profile_fail_before_publication() {
    let directory = tempfile::tempdir().expect("valid test fixture");
    let mut manifest = setup(directory.path());
    manifest["timeline_sha256"] = json!("changed");
    save(directory.path(), &manifest);
    command(directory.path()).assert().failure();
    assert!(!directory.path().join("report.json").exists());
    let manifest = setup(directory.path());
    save(directory.path(), &manifest);
    let profile_file = directory.path().join("profile.json");
    let mut profile: Value =
        serde_json::from_slice(&fs::read(&profile_file).expect("valid test fixture"))
            .expect("valid test fixture");
    profile["segments"][0]["targets"]["B528"]["lead_ms"] = json!(0);
    fs::write(
        &profile_file,
        serde_json::to_vec(&profile).expect("valid test fixture"),
    )
    .expect("valid test fixture");
    command(directory.path()).assert().failure();
    assert!(!directory.path().join("report.json").exists());
}

#[test]
fn entirely_excluded_inputs_do_not_pass() {
    let directory = tempfile::tempdir().expect("valid test fixture");
    let mut manifest = setup(directory.path());
    manifest["inputs"] = json!([{"input":"missing.json","sha256":"unused"}]);
    save(directory.path(), &manifest);
    command(directory.path()).assert().failure();
    let report: Value = serde_json::from_slice(
        &fs::read(directory.path().join("report.json")).expect("valid test fixture"),
    )
    .expect("valid test fixture");
    assert_eq!(report["summary"]["eligible"], 0);
    assert_eq!(report["summary"]["passed"], false);
}

#[test]
fn a_scope_with_no_matching_targets_is_an_error_instead_of_an_empty_pass() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let manifest = setup(directory.path());
    save(directory.path(), &manifest);
    let path = directory.path().join("profile.json");
    let mut profile: Value =
        serde_json::from_slice(&fs::read(&path).expect("profile")).expect("JSON");
    profile["segments"][0]["targets"] = json!({"FFFF":{"name":"Never observed"}});
    fs::write(path, serde_json::to_vec(&profile).expect("JSON")).expect("profile");
    command(directory.path()).assert().failure();
    let report: Value =
        serde_json::from_slice(&fs::read(directory.path().join("report.json")).expect("report"))
            .expect("JSON");
    assert_eq!(report["summary"]["eligible"], 1);
    assert_eq!(report["summary"]["passed"], false);
    assert_eq!(report["traces"][0]["status"], "error");
}
