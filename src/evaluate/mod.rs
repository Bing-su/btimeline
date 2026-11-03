//! Shared authored-timeline evaluation. Profiles describe observations, never executable game rules.
mod audit;
mod coverage;
mod evidence;
mod matching;
mod projection;
mod review;
mod score;
#[cfg(test)]
mod tests;

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result, ensure};
use camino::Utf8PathBuf;
pub use evidence::EvidenceCommand;
use projection::{InputExclusion, Profile, UnverifiedScope, exclude};
pub use review::ReviewCommand;
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
    profile: Option<Utf8PathBuf>,
    /// New JSON report; never overwrite an existing result.
    #[usage(short = 'o', long)]
    output: Utf8PathBuf,
}
#[derive(Deserialize)]
struct Manifest {
    timeline: String,
    timeline_sha256: String,
    role: String,
    evidence_role: Option<coverage::EvidenceRole>,
    forecast_lead_ms: i64,
    horizon_ms: i64,
    tolerance_ms: i64,
    profile_sha256: Option<String>,
    evaluator_sha256: Option<String>,
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
    profile: Option<&Profile>,
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
    let Some(profile) = profile else {
        return Ok(
            json!({"status":"unverified","input":item.input,"sha256":item.sha256,"passed":null,"review_required":true,"error":"No phase with a supported literal cast entry in KDL"}),
        );
    };
    *stage = "input-validation";
    let projected = projection::project(&log, profile, Some(model))?;
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
    let fingerprint = if profile.inferred {
        let phases = projected
            .bosses
            .iter()
            .map(|boss| &boss.phase)
            .collect::<Vec<_>>();
        hash(&serde_json::to_vec(&(phases, &samples))?)
    } else {
        hash(&serde_json::to_vec(&samples)?)
    };
    if samples.iter().any(|sample| !sample.is_empty())
        && (manifest.excluded_fingerprints.contains(&fingerprint)
            || !seen.insert(fingerprint.clone()))
    {
        // Duplicate target sequences may contain new unselected casts; retain the audit without counting independence.
        let audit_only = projected
            .bosses
            .iter()
            .map(|boss| json!({"phase":boss.phase,"audit":boss.audit}))
            .collect::<Vec<_>>();
        return Err(exclude(
            "Duplicate mechanic sequence",
            json!({"fingerprint":fingerprint,"audit_only":audit_only,"review_required":projected.bosses.iter().any(|boss|boss.audit.unclassified_casts > 0)}),
        ));
    }
    *stage = "timeline-evaluation";
    let mut result = score::score(model, &projected, manifest)?;
    let unverified_phases = projected
        .bosses
        .iter()
        .filter(|boss| {
            !boss.targets.iter().any(|target| {
                target.at_ms - boss.entry_ms >= target.lead_ms.unwrap_or(manifest.forecast_lead_ms)
            })
        })
        .map(|boss| &boss.phase)
        .collect::<Vec<_>>();
    let fields = result
        .as_object_mut()
        .context("Expected evaluation object")?;
    if profile.inferred && !unverified_phases.is_empty() {
        fields.insert("unverified_phases".into(), json!(unverified_phases));
        fields.insert("review_required".into(), json!(true));
    }
    let status = if profile.inferred
        && !unverified_phases.is_empty()
        && fields.get("passed") == Some(&json!(true))
    {
        fields.insert("passed".into(), Value::Null);
        fields.insert(
            "error".into(),
            json!("At least one phase has no inferred targets with enough observed time for a forecast"),
        );
        "unverified"
    } else {
        "evaluated"
    };
    let fight = log.report.fights.first().context("Missing fight")?;
    fields.extend(json!({"status":status,"input":item.input,"sha256":item.sha256,"trace":format!("{}/{}",log.report.code,fight.id),"fingerprint":fingerprint}).as_object().context("Trace fields")?.clone());
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
    let unverified = eligible
        .iter()
        .filter(|trace| trace.get("status").and_then(Value::as_str) == Some("unverified"))
        .count();
    let mut result = json!({"selected":traces.len(),"excluded":traces.len()-eligible.len(),"eligible":eligible.len()-unverified,"passed_traces":passed,"passed":!eligible.is_empty()&&passed==eligible.len()});
    if let Some(fields) = result.as_object_mut().filter(|_| unverified > 0) {
        fields.insert("unverified".into(), json!(unverified));
    }
    result
}

impl Manifest {
    fn validate_criteria(&self, explicit_profile: bool) -> Result<()> {
        ensure!(
            self.evidence_role != Some(coverage::EvidenceRole::HeldOut)
                || (self.evaluator_sha256.is_some()
                    && (!explicit_profile || self.profile_sha256.is_some())),
            "Held-out coverage requires frozen profile and evaluator hashes when a profile is supplied, and a frozen evaluator otherwise"
        );
        ensure!(
            (1..=crate::timeline::MAX_MS).contains(&self.forecast_lead_ms)
                && (1..=crate::timeline::MAX_MS).contains(&self.horizon_ms)
                && (0..=crate::timeline::MAX_MS).contains(&self.tolerance_ms),
            "Invalid forecast criteria"
        );
        Ok(())
    }
}

fn failure(item: &Input, stage: &str, error: anyhow::Error) -> Value {
    if let Some(scope) = error.downcast_ref::<UnverifiedScope>() {
        json!({"status":"unverified","input":item.input,"sha256":item.sha256,"stage":stage,"error":scope.to_string(),"passed":null,"review_required":true,"evidence":scope.1})
    } else if let Some(exclusion) = error.downcast_ref::<InputExclusion>() {
        json!({"status":"excluded","input":item.input,"sha256":item.sha256,"stage":stage,"error":format!("InputExclusion: {exclusion}"),"passed":null,"evidence":exclusion.evidence,"review_required":exclusion.evidence.get("review_required")})
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
        manifest.validate_criteria(self.profile.is_some())?;
        let profile_bytes = self.profile.as_ref().map(fs::read).transpose()?;
        let profile_sha256 = profile_bytes.as_deref().map(hash);
        let evaluator_sha256 = hash(&fs::read(std::env::current_exe()?)?);
        ensure!(
            manifest
                .profile_sha256
                .as_ref()
                .is_none_or(|expected| Some(expected) == profile_sha256.as_ref()),
            "Projection profile changed since freeze"
        );
        ensure!(
            manifest
                .evaluator_sha256
                .as_ref()
                .is_none_or(|expected| expected == &evaluator_sha256),
            "Evaluator binary changed since freeze"
        );
        let timeline_bytes = fs::read(&manifest.timeline)?;
        ensure!(
            hash(&timeline_bytes) == manifest.timeline_sha256,
            "Candidate changed"
        );
        let model = crate::timeline::parse(std::str::from_utf8(&timeline_bytes)?)?;
        let profile = if let Some(bytes) = &profile_bytes {
            Some(serde_json::from_slice::<Profile>(bytes)?)
        } else {
            Profile::infer(&model)
        };
        if let Some(profile) = &profile {
            profile.validate()?;
            profile.validate_bindings(&model)?;
        }
        let mut seen = BTreeSet::new();
        let traces = manifest
            .inputs
            .iter()
            .map(|item| {
                let mut stage = "input-validation";
                evaluate_input(
                    item,
                    profile.as_ref(),
                    &model,
                    &manifest,
                    &mut seen,
                    &mut stage,
                )
                .unwrap_or_else(|error| failure(item, stage, error))
            })
            .collect::<Vec<_>>();
        // The run uses immutable in-memory inputs; bind the report to files that stayed unchanged, too.
        for (path, bytes) in [
            (self.manifest.as_str(), &manifest_bytes),
            (manifest.timeline.as_str(), &timeline_bytes),
        ] {
            ensure_unchanged(path, bytes)?;
        }
        if let Some((path, bytes)) = self.profile.as_ref().zip(profile_bytes.as_ref()) {
            ensure_unchanged(path.as_str(), bytes)?;
        }
        let summary = summary(&traces);
        let passed = summary.get("passed") == Some(&Value::Bool(true));
        let mut not_checked = vec![
            "ACT equivalence and actual lifecycle signals".to_owned(),
            "UI visibility".to_owned(),
        ];
        if let Some(profile) = profile {
            not_checked.extend(profile.not_checked);
        }
        let review_required = traces
            .iter()
            .filter_map(|trace| trace.get("review_required").and_then(Value::as_bool))
            .reduce(|a, b| a || b);
        let report = json!({
            "version": 1, "manifest": self.manifest.as_str(), "manifest_sha256": hash(&manifest_bytes),
            "timeline_sha256": manifest.timeline_sha256, "profile": self.profile.as_ref().map(|path| path.as_str()),
            "projection": if self.profile.is_some() {"profile"} else {"inferred"},
            "profile_sha256": profile_sha256, "evaluator_sha256": evaluator_sha256,
            "bindings": { "profile_frozen": manifest.profile_sha256.is_some(), "evaluator_frozen": manifest.evaluator_sha256.is_some() },
            "role": manifest.role, "summary": summary, "review_required": review_required,
            "evidence_role": manifest.evidence_role.unwrap_or_default(),
            "traces": traces, "not_checked": not_checked
        });
        crate::output::write_new(&[(
            self.output.as_std_path(),
            &serde_json::to_vec_pretty(&report)?,
        )])?;
        println!(
            "{}",
            json!({"summary": summary, "review_required": review_required, "report": self.output.as_str()})
        );
        ensure!(passed, "Evaluation failures saved: {}", self.output);
        Ok(())
    }
}
