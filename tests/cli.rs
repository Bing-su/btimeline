use std::fs;

use assert_cmd::cargo::cargo_bin_cmd;
use serde_json::Value;

const INPUT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/pull.json");

#[test]
fn help_version_and_invalid_arguments_use_process_exit_codes() {
    let directory = tempfile::tempdir().expect("temporary output directory");
    let output = directory.path().join("unused.yaml");
    cargo_bin_cmd!("btimeline").arg("--help").assert().success();
    let version = cargo_bin_cmd!("btimeline")
        .arg("--version")
        .assert()
        .success();
    assert!(
        String::from_utf8_lossy(&version.get_output().stdout).contains(env!("CARGO_PKG_VERSION"))
    );
    cargo_bin_cmd!("btimeline")
        .args(["generate", INPUT, "-o"])
        .arg(&output)
        .args(["--mode", "invalid"])
        .assert()
        .code(2);
    assert!(!output.exists());
}

#[test]
fn piped_inspection_keeps_stdout_json_and_progress_hidden() {
    let result = cargo_bin_cmd!("btimeline")
        .args(["inspect-inputs", INPUT])
        .assert()
        .success()
        .stderr("");
    let groups: Value = serde_json::from_slice(&result.get_output().stdout).expect("JSON stdout");
    assert_eq!(
        groups.pointer("/0/pulls/0/report"),
        Some(&Value::from("synthetic"))
    );
}

#[test]
fn fflogs_tools_load_dotenv_without_exposing_credentials() {
    let directory = tempfile::tempdir().expect("temporary credentials directory");
    fs::write(
        directory.path().join(".env"),
        "FFLOGS_CLIENT_ID=test-id\nFFLOGS_CLIENT_SECRET='private-test-value'\n",
    )
    .expect("local credentials fixture");
    for arguments in [
        vec!["fflogs", "invalid/code", "1"],
        vec!["fflogs-collect", "-o", "plan.json"],
    ] {
        let result = cargo_bin_cmd!("btimeline")
            .current_dir(directory.path())
            .env_remove("FFLOGS_CLIENT_ID")
            .env_remove("FFLOGS_CLIENT_SECRET")
            .args(arguments)
            .assert()
            .failure();
        let stderr = String::from_utf8_lossy(&result.get_output().stderr);
        assert!(!stderr.contains("private-test-value"));
        assert!(!stderr.contains("Missing required flag"));
    }
    // A malformed secret is never included in a dotenv parser error.
    fs::write(
        directory.path().join(".env"),
        "FFLOGS_CLIENT_SECRET='private-test-value\n",
    )
    .expect("malformed fixture");
    let result = cargo_bin_cmd!("btimeline")
        .current_dir(directory.path())
        .args(["fflogs-collect", "--help"])
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&result.get_output().stderr);
    assert!(stderr.contains("Cannot load .env"));
    assert!(!stderr.contains("private-test-value"));
}

#[test]
fn prepare_replays_all_casts_and_converts_only_when_requested() {
    for convert in [false, true] {
        let directory = tempfile::tempdir().expect("temporary output directory");
        let output = directory.path().join("draft.yaml");
        let mut command = cargo_bin_cmd!("btimeline");
        command.args(["prepare", INPUT, "-o"]).arg(&output);
        if convert {
            command.arg("--convert");
        }
        command.assert().success();
        let yaml = fs::read_to_string(&output).expect("generated YAML");
        let timeline: Value =
            serde_saphyr::from_str(&yaml).expect("valid YAML after major upgrade");
        assert_eq!(timeline.get("schemaVersion"), Some(&Value::from(1)));
        let replay: Value = serde_json::from_slice(
            &fs::read(output.with_extension("replay.json")).expect("replay report"),
        )
        .expect("valid replay JSON");
        assert_eq!(
            replay.get("status"),
            Some(&Value::from("internally_validated"))
        );
        assert_eq!(
            replay.pointer("/pulls/0/replay/summary/matches"),
            Some(&Value::from(3))
        );
        assert_eq!(
            replay.pointer("/pulls/0/unrepresentedEventIndices"),
            Some(&Value::from(Vec::<usize>::new()))
        );
        assert_eq!(output.with_extension("txt").exists(), convert);

        // Fail before publishing anything, e.g. rerunning prepare preserves every existing artifact.
        let previous = fs::read(&output).expect("existing draft");
        cargo_bin_cmd!("btimeline")
            .args(["prepare", INPUT, "-o"])
            .arg(&output)
            .assert()
            .failure();
        assert_eq!(fs::read(&output).expect("preserved draft"), previous);
    }
}
