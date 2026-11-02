//! Shared authored-timeline evaluation. Profiles describe observations, never executable game rules.
mod projection;
mod score;
#[cfg(test)]
mod tests;

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result, ensure};
use camino::Utf8PathBuf;
use projection::{InputExclusion, Profile, exclude};
use ring::digest::{SHA256, digest};
use serde::Deserialize;
use serde_json::{Value, json};
use usage_rs::{Args, Run};

#[derive(Args)]
pub struct EvaluateCommand {
    /// Frozen input manifest (timeline, hashes, role, targets' forecast criteria).
    #[usage(arg)]
    manifest: Utf8PathBuf,
    /// Encounter-specific projection facts and scope, not a runtime timeline dependency.
    #[usage(long)]
    profile: Utf8PathBuf,
    /// New JSON report; never overwrite an existing result.
    #[usage(short = 'o', long)]
    output: Utf8PathBuf,
}
#[derive(Deserialize)]
struct Manifest {
    timeline: String,
    timeline_sha256: String,
    role: String,
    forecast_lead_ms: i64,
    horizon_ms: i64,
    tolerance_ms: i64,
    #[serde(default)]
    excluded_fingerprints: BTreeSet<String>,
    inputs: Vec<Input>,
}
#[derive(Deserialize)]
struct Input {
    input: String,
    sha256: String,
}
fn hash(bytes: &[u8]) -> String {
    digest(&SHA256, bytes)
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn ensure_unchanged(path: &str, original: &[u8]) -> Result<()> {
    ensure!(
        fs::read(path)? == original,
        "Evaluation source changed during evaluation: {path}"
    );
    Ok(())
}

fn evaluate_input(
    item: &Input,
    profile: &Profile,
    model: &crate::timeline::Timeline,
    manifest: &Manifest,
    seen: &mut BTreeSet<String>,
    stage: &mut &str,
) -> Result<Value> {
    if !Path::new(&item.input).exists() {
        return Err(exclude("Missing input", json!({"input":item.input})));
    }
    let bytes = fs::read(&item.input)?;
    ensure!(hash(&bytes) == item.sha256, "Input changed since selection");
    *stage = "input-inspection";
    // Reuse inspect's integrity validation; a malformed download is an error, not an exclusion.
    let log = crate::generate::validated_collected(Path::new(&item.input), &bytes)?;
    *stage = "input-validation";
    let projected = projection::project(&log, profile)?;
    let samples = projected
        .bosses
        .iter()
        .map(|boss| {
            boss.targets
                .iter()
                .map(|target| (target.at_ms - boss.entry_ms, &target.id))
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    let fingerprint = hash(&serde_json::to_vec(&samples)?);
    if manifest.excluded_fingerprints.contains(&fingerprint) || !seen.insert(fingerprint.clone()) {
        return Err(exclude(
            "Duplicate mechanic sequence",
            json!({"fingerprint":fingerprint}),
        ));
    }
    *stage = "timeline-evaluation";
    let mut result = score::score(model, &projected, manifest)?;
    let fields = result
        .as_object_mut()
        .context("Expected evaluation object")?;
    let fight = log.report.fights.first().context("Missing fight")?;
    fields.extend(json!({"status":"evaluated","input":item.input,"sha256":item.sha256,"trace":format!("{}/{}",log.report.code,fight.id),"fingerprint":fingerprint}).as_object().context("Trace fields")?.clone());
    Ok(result)
}
fn summary(traces: &[Value]) -> Value {
    let eligible = traces
        .iter()
        .filter(|trace| trace.get("status").and_then(Value::as_str) != Some("excluded"))
        .collect::<Vec<_>>();
    let passed = eligible
        .iter()
        .filter(|trace| trace.get("passed") == Some(&Value::Bool(true)))
        .count();
    json!({"selected":traces.len(),"excluded":traces.len()-eligible.len(),"eligible":eligible.len(),"passed_traces":passed,"passed":!eligible.is_empty()&&passed==eligible.len()})
}

fn failure(item: &Input, stage: &str, error: anyhow::Error) -> Value {
    if let Some(exclusion) = error.downcast_ref::<InputExclusion>() {
        json!({"status":"excluded","input":item.input,"sha256":item.sha256,"stage":stage,"error":format!("InputExclusion: {exclusion}"),"passed":null,"evidence":exclusion.evidence})
    } else {
        json!({"status":"error","input":item.input,"sha256":item.sha256,"stage":stage,"error":format!("{error:#}"),"passed":false})
    }
}

impl Run for EvaluateCommand {
    type Output = Result<()>;

    fn run(self) -> Result<()> {
        ensure!(
            !self.output.exists(),
            "Report already exists: {}",
            self.output
        );
        let manifest_bytes = fs::read(&self.manifest)?;
        let manifest: Manifest = serde_json::from_slice(&manifest_bytes)?;
        ensure!(
            (1..=crate::timeline::MAX_MS).contains(&manifest.forecast_lead_ms)
                && (1..=crate::timeline::MAX_MS).contains(&manifest.horizon_ms)
                && (0..=crate::timeline::MAX_MS).contains(&manifest.tolerance_ms),
            "Invalid forecast criteria"
        );
        let profile_bytes = fs::read(&self.profile)?;
        let profile: Profile = serde_json::from_slice(&profile_bytes)?;
        profile.validate()?;
        let timeline_bytes = fs::read(&manifest.timeline)?;
        ensure!(
            hash(&timeline_bytes) == manifest.timeline_sha256,
            "Candidate changed"
        );
        let model = crate::timeline::parse(std::str::from_utf8(&timeline_bytes)?)?;
        let mut seen = BTreeSet::new();
        let traces = manifest
            .inputs
            .iter()
            .map(|item| {
                let mut stage = "input-validation";
                evaluate_input(item, &profile, &model, &manifest, &mut seen, &mut stage)
                    .unwrap_or_else(|error| failure(item, stage, error))
            })
            .collect::<Vec<_>>();
        // The run uses immutable in-memory inputs; bind the report to files that stayed unchanged, too.
        for (path, bytes) in [
            (self.manifest.as_str(), &manifest_bytes),
            (self.profile.as_str(), &profile_bytes),
            (manifest.timeline.as_str(), &timeline_bytes),
        ] {
            ensure_unchanged(path, bytes)?;
        }
        let summary = summary(&traces);
        let passed = summary.get("passed") == Some(&Value::Bool(true));
        let mut not_checked = vec![
            "ACT equivalence and actual lifecycle signals".to_owned(),
            "UI visibility".to_owned(),
        ];
        not_checked.extend(profile.not_checked);
        let report = json!({"manifest":self.manifest.as_str(),"manifest_sha256":hash(&manifest_bytes),"timeline_sha256":manifest.timeline_sha256,"profile":self.profile.as_str(),"profile_sha256":hash(&profile_bytes),"role":manifest.role,"summary":summary,"traces":traces,"not_checked":not_checked});
        crate::output::write_new(&[(
            self.output.as_std_path(),
            &serde_json::to_vec_pretty(&report)?,
        )])?;
        println!("{summary}");
        ensure!(passed, "Evaluation failures saved: {}", self.output);
        Ok(())
    }
}
