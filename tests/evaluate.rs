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

#[test]
fn unselected_casts_remain_auditable_and_evidence_and_review_are_bounded() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let mut manifest = setup(directory.path());
    let input = directory.path().join("input.json");
    let mut raw: Value = serde_json::from_slice(&fs::read(&input).expect("input")).expect("JSON");
    raw["report"]["masterData"]["abilities"]
        .as_array_mut()
        .expect("abilities")
        .extend([
            json!({"gameID":65535,"name":"New mechanic","type":"1"}),
            json!({"gameID":61166,"name":"Follow-through","type":"1"}),
        ]);
    raw["events"].as_array_mut().expect("events").extend([
        json!({"timestamp":12000,"type":"cast","sourceID":10,"abilityGameID":65535,"fight":1}),
        json!({"timestamp":16000,"type":"cast","sourceID":10,"abilityGameID":61166,"fight":1}),
        json!({"timestamp":17000,"type":"cast","sourceID":10,"abilityGameID":65535,"fight":1}),
    ]);
    raw["collection"]["eventCount"] = json!(6);
    let bytes = serde_json::to_vec(&raw).expect("JSON");
    fs::write(&input, &bytes).expect("input");
    manifest["inputs"][0]["sha256"] = json!(hash(&bytes));
    save(directory.path(), &manifest);
    let profile_path = directory.path().join("profile.json");
    let mut profile: Value =
        serde_json::from_slice(&fs::read(&profile_path).expect("profile")).expect("JSON");
    profile["segments"][0]["ignored"] =
        json!({"EEEE":"Summarized follow-through; target bars are unchanged"});
    fs::write(&profile_path, serde_json::to_vec(&profile).expect("JSON")).expect("profile");
    command(directory.path()).assert().success();
    let report: Value =
        serde_json::from_slice(&fs::read(directory.path().join("report.json")).expect("report"))
            .expect("JSON");
    assert_eq!(report["summary"]["passed"], true);
    assert_eq!(report["review_required"], true);
    let audit = &report["traces"][0]["bosses"][0]["audit"];
    assert_eq!(audit["observed_casts"], 6);
    assert_eq!(audit["target_casts"], 3);
    assert_eq!(audit["ignored_casts"], 1);
    assert_eq!(audit["unclassified_casts"], 2);
    assert_eq!(audit["unclassified"][0]["first_raw_index"], 3);
    assert_eq!(audit["unclassified"][0]["last_raw_index"], 5);
    let output = cargo_bin_cmd!("btimeline")
        .current_dir(directory.path())
        .args([
            "evidence",
            "input.json",
            "--around-index",
            "4",
            "--before-ms",
            "5000",
            "--after-ms",
            "2000",
            "--limit",
            "2",
        ])
        .assert()
        .success();
    let evidence: Value =
        serde_json::from_slice(&output.get_output().stdout).expect("evidence JSON");
    assert_eq!(evidence["total_matches"], 3);
    assert_eq!(evidence["omitted"], 1);
    assert_eq!(evidence["rows"][0][0], 4);
    assert_eq!(evidence["rows"][1][0], 5);
    assert_eq!(evidence["rows"][0][1], 15000);
    let output = cargo_bin_cmd!("btimeline")
        .current_dir(directory.path())
        .args(["review", "report.json", "--limit", "1"])
        .assert()
        .success();
    let review: Value = serde_json::from_slice(&output.get_output().stdout).expect("review JSON");
    assert_eq!(review["review"]["audit_status"], "needs_review");
    assert_eq!(review["review"]["unclassified"][0]["raw_index"], 3);
    assert_eq!(review["review"]["phases"][0]["forecasts"], 1);
}

#[test]
fn profile_and_evaluator_can_be_frozen_without_rewriting_historical_manifests() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let mut manifest = setup(directory.path());
    let profile_path = directory.path().join("profile.json");
    let profile = fs::read(&profile_path).expect("profile");
    manifest["profile_sha256"] = json!(hash(&profile));
    manifest["evaluator_sha256"] = json!(hash(
        &fs::read(env!("CARGO_BIN_EXE_btimeline")).expect("binary")
    ));
    save(directory.path(), &manifest);
    command(directory.path()).assert().success();
    let report_path = directory.path().join("report.json");
    let report: Value =
        serde_json::from_slice(&fs::read(&report_path).expect("report")).expect("JSON");
    assert_eq!(
        report["bindings"],
        json!({"profile_frozen":true,"evaluator_frozen":true})
    );
    fs::remove_file(&report_path).expect("remove report");
    let mut edited = profile.clone();
    edited.push(b' ');
    fs::write(&profile_path, edited).expect("edit profile");
    let failure = command(directory.path()).assert().failure();
    assert!(
        String::from_utf8_lossy(&failure.get_output().stderr)
            .contains("Projection profile changed")
    );
    assert!(!report_path.exists());
    fs::write(&profile_path, profile).expect("restore profile");
    manifest["evaluator_sha256"] = json!("different binary");
    save(directory.path(), &manifest);
    let failure = command(directory.path()).assert().failure();
    assert!(
        String::from_utf8_lossy(&failure.get_output().stderr).contains("Evaluator binary changed")
    );
    assert!(!report_path.exists());
}

#[test]
fn ignored_ids_require_reasons_and_cannot_hide_targets() {
    for ignored in [json!({"EEEE":" "}), json!({"B528":"Hide a scored target"})] {
        let directory = tempfile::tempdir().expect("temporary directory");
        let manifest = setup(directory.path());
        save(directory.path(), &manifest);
        let path = directory.path().join("profile.json");
        let mut profile: Value =
            serde_json::from_slice(&fs::read(&path).expect("profile")).expect("JSON");
        profile["segments"][0]["ignored"] = ignored;
        fs::write(path, serde_json::to_vec(&profile).expect("JSON")).expect("profile");
        command(directory.path()).assert().failure();
        assert!(!directory.path().join("report.json").exists());
    }
}

#[test]
fn legacy_reviews_bound_each_list_and_never_claim_an_absent_audit_was_complete() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let targets = json!([
        {"raw_index":5,"at_ms":1000,"id":"A001","error_ms":null,"forecast":null,"passed":false},
        {"raw_index":6,"at_ms":2000,"id":"A002","error_ms":null,"forecast":null,"passed":false}
    ]);
    let mut report = json!({
        "summary":{"selected":3,"excluded":0,"eligible":3,"passed_traces":0,"passed":false},
        "traces":(0..3).map(|index|json!({"input":format!("input-{index}"),"status":"evaluated","passed":false,"bosses":[{"phase":format!("phase-{index}"),"targets":targets}]})).collect::<Vec<_>>()
    });
    let path = directory.path().join("report.json");
    fs::write(&path, serde_json::to_vec(&report).expect("JSON")).expect("report");
    let output = cargo_bin_cmd!("btimeline")
        .current_dir(directory.path())
        .args(["review", "report.json", "--limit", "1"])
        .assert()
        .success();
    let review: Value = serde_json::from_slice(&output.get_output().stdout).expect("JSON");
    assert_eq!(
        review["review"]["phases"].as_array().expect("phases").len(),
        1
    );
    assert_eq!(review["review"]["omitted_phases"], 2);
    assert_eq!(
        review["review"]["issues"].as_array().expect("issues").len(),
        1
    );
    assert_eq!(review["review"]["omitted_issues"], 2);
    assert_eq!(review["review"]["issues"][0]["raw_index"], 5);
    assert_eq!(review["review"]["audit_status"], "unavailable_or_partial");
    report["summary"]["passed"] = json!(true);
    fs::write(&path, serde_json::to_vec(&report).expect("JSON")).expect("report");
    cargo_bin_cmd!("btimeline")
        .current_dir(directory.path())
        .args(["review", "report.json"])
        .assert()
        .failure();
}

#[test]
fn evidence_includes_unregistered_helpers_and_validates_bounds() {
    let directory = tempfile::tempdir().expect("temporary directory");
    setup(directory.path());
    let path = directory.path().join("input.json");
    let mut raw: Value = serde_json::from_slice(&fs::read(&path).expect("input")).expect("JSON");
    raw["report"]["masterData"]["actors"].as_array_mut().expect("actors").push(json!({"id":11,"name":"Unreported Helper","gameID":99002,"type":"NPC","subType":"Unknown"}));
    raw["events"].as_array_mut().expect("events").push(
        json!({"timestamp":12000,"type":"cast","sourceID":11,"abilityGameID":46376,"fight":1}),
    );
    raw["collection"]["eventCount"] = json!(4);
    fs::write(path, serde_json::to_vec(&raw).expect("JSON")).expect("input");
    let output = cargo_bin_cmd!("btimeline")
        .current_dir(directory.path())
        .args(["evidence", "input.json", "--source", "Unreported Helper"])
        .assert()
        .success();
    let evidence: Value = serde_json::from_slice(&output.get_output().stdout).expect("JSON");
    assert_eq!(evidence["total_matches"], 1);
    assert_eq!(evidence["rows"][0][0], 3);
    for args in [
        ["--around-index", "999"],
        ["--limit", "0"],
        ["--before-ms", "-1"],
    ] {
        cargo_bin_cmd!("btimeline")
            .current_dir(directory.path())
            .args(["evidence", "input.json"])
            .args(args)
            .assert()
            .failure();
    }
}

#[test]
fn duplicate_target_sequences_do_not_discard_new_unclassified_observations() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let mut manifest = setup(directory.path());
    let mut raw: Value =
        serde_json::from_slice(include_bytes!("fixtures/pull.json")).expect("JSON");
    raw["report"]["masterData"]["abilities"]
        .as_array_mut()
        .expect("abilities")
        .push(json!({"gameID":65535,"name":"New mechanic","type":"1"}));
    raw["events"].as_array_mut().expect("events").push(
        json!({"timestamp":12000,"type":"cast","sourceID":10,"abilityGameID":65535,"fight":1}),
    );
    raw["collection"]["eventCount"] = json!(4);
    let bytes = serde_json::to_vec(&raw).expect("JSON");
    fs::write(directory.path().join("variant.json"), &bytes).expect("variant");
    manifest["inputs"]
        .as_array_mut()
        .expect("inputs")
        .push(json!({"input":"variant.json","sha256":hash(&bytes)}));
    save(directory.path(), &manifest);
    command(directory.path()).assert().success();
    let report: Value =
        serde_json::from_slice(&fs::read(directory.path().join("report.json")).expect("report"))
            .expect("JSON");
    assert_eq!(report["summary"]["eligible"], 1);
    assert_eq!(report["traces"][1]["status"], "excluded");
    assert_eq!(report["review_required"], true);
    assert_eq!(
        report["traces"][1]["evidence"]["audit_only"][0]["audit"]["unclassified"][0]["first_raw_index"],
        3
    );
    let output = cargo_bin_cmd!("btimeline")
        .current_dir(directory.path())
        .args(["review", "report.json"])
        .assert()
        .success();
    let review: Value = serde_json::from_slice(&output.get_output().stdout).expect("JSON");
    assert_eq!(review["review"]["audit_status"], "needs_review");
    assert_eq!(
        review["review"]["unclassified"][0]["example_input"],
        "variant.json"
    );
}

#[test]
fn review_prioritizes_failures_over_exclusions_and_clips_error_text() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let report = json!({
        "summary":{"selected":2,"excluded":1,"eligible":1,"passed_traces":0,"passed":false},
        "traces":[
            {"input":"missing","status":"excluded","passed":null,"error":"Missing"},
            {"input":"broken","status":"error","passed":false,"error":"오류".repeat(3000)}
        ]
    });
    fs::write(
        directory.path().join("report.json"),
        serde_json::to_vec(&report).expect("JSON"),
    )
    .expect("report");
    let output = cargo_bin_cmd!("btimeline")
        .current_dir(directory.path())
        .args(["review", "report.json", "--limit", "1"])
        .assert()
        .success();
    let review: Value = serde_json::from_slice(&output.get_output().stdout).expect("JSON");
    assert_eq!(review["review"]["issues"][0]["input"], "broken");
    assert_eq!(
        review["review"]["issues"][0]["error"]
            .as_str()
            .expect("error")
            .chars()
            .count(),
        400
    );
    assert_eq!(review["review"]["omitted_issues"], 1);
}

#[test]
fn unclassified_groups_can_be_paged_without_unbounded_output() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let report = json!({
        "version":1,"summary":{"selected":1,"excluded":0,"eligible":1,"passed_traces":1,"passed":true},
        "traces":[{"input":"source.json","status":"evaluated","passed":true,"bosses":[{
            "phase":"boss","targets":[{"raw_index":0,"at_ms":0,"id":"A001","error_ms":0,"forecast":null,"passed":true}],
            "audit":{"declared_sources":["Boss"],"observed_casts":4,"target_casts":1,"ignored_casts":0,"unclassified_casts":3,
                "unclassified":(2..5).map(|n|json!({"source":"Boss","id":format!("A00{n}"),"name":"Unknown","count":1,"first_raw_index":n,"last_raw_index":n,"first_at_ms":n*1000,"last_at_ms":n*1000})).collect::<Vec<_>>()}
        }]}]
    });
    fs::write(
        directory.path().join("report.json"),
        serde_json::to_vec(&report).expect("JSON"),
    )
    .expect("report");
    for (offset, id, next) in [("1", "A003", json!(2)), ("2", "A004", Value::Null)] {
        let output = cargo_bin_cmd!("btimeline")
            .current_dir(directory.path())
            .args([
                "review",
                "report.json",
                "--limit",
                "1",
                "--unclassified-offset",
                offset,
            ])
            .assert()
            .success();
        let result: Value = serde_json::from_slice(&output.get_output().stdout).expect("JSON");
        assert_eq!(
            result["review"]["unclassified"]
                .as_array()
                .expect("rows")
                .len(),
            1
        );
        assert_eq!(result["review"]["unclassified"][0]["id"], id);
        assert_eq!(result["review"]["omitted_unclassified"], 2);
        assert_eq!(result["review"]["next_unclassified_offset"], next);
    }
}
