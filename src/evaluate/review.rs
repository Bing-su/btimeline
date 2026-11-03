//! Read saved results without replaying, and cap every example list shown to the author.
use std::collections::BTreeMap;
use std::fs;

use anyhow::{Context, Result, ensure};
use camino::Utf8PathBuf;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use usage_rs::{Args, Run};

use super::audit::Audit;
mod coverage;

#[derive(Args)]
pub struct ReviewCommand {
    #[usage(arg)]
    report: Utf8PathBuf,
    /// Maximum rows per summary list (1..50).
    #[usage(long, default = "8")]
    limit: usize,
    /// Page through unclassified groups without expanding the output limit; other lists are unchanged.
    #[usage(long, default = "0")]
    unclassified_offset: usize,
}
#[derive(Deserialize)]
struct Report {
    version: Option<u32>,
    summary: Value,
    profile_sha256: Option<String>,
    evaluator_sha256: Option<String>,
    bindings: Option<Value>,
    traces: Vec<Trace>,
    evidence_role: Option<super::coverage::EvidenceRole>,
}
#[derive(Deserialize)]
struct Trace {
    input: String,
    trace: Option<String>,
    fight_duration_ms: Option<i64>,
    status: String,
    passed: Option<bool>,
    stage: Option<String>,
    error: Option<String>,
    evidence: Option<ExclusionEvidence>,
    safe_entry: Option<bool>,
    entry_mismatch: Option<Value>,
    #[serde(default)]
    diagnostics: Vec<Value>,
    #[serde(default)]
    bosses: Vec<Boss>,
    coverage: Option<super::coverage::Coverage>,
    #[serde(default)]
    unverified_phases: Vec<String>,
}
#[derive(Deserialize)]
struct Boss {
    phase: String,
    entry_ms: Option<i64>,
    end_ms: Option<i64>,
    targets: Vec<Target>,
    audit: Option<Audit>,
    #[serde(default)]
    unmatched_displays: Vec<Value>,
}
#[derive(Deserialize)]
struct ExclusionEvidence {
    #[serde(default)]
    audit_only: Vec<AuditScope>,
    entry_candidates: Option<Vec<EntryCandidate>>,
}
#[derive(Deserialize)]
struct EntryCandidate {
    phase: String,
    raw_index: usize,
}
#[derive(Deserialize)]
struct AuditScope {
    phase: String,
    audit: Audit,
}
#[derive(Deserialize)]
struct Target {
    raw_index: usize,
    at_ms: i64,
    id: String,
    #[serde(default)]
    event_ids: Vec<String>,
    matched_event: Option<Value>,
    error_ms: Option<i64>,
    forecast: Option<Forecast>,
    passed: bool,
    #[serde(default)]
    failure_reasons: Vec<String>,
}
#[derive(Deserialize)]
struct Forecast {
    error_ms: Option<i64>,
    lead_ms: Option<i64>,
    passed: bool,
    #[serde(default)]
    diagnostics: Vec<Value>,
    #[serde(default)]
    current_diagnostics: Vec<Value>,
    #[serde(default)]
    projection_diagnostics: Vec<Value>,
}
#[derive(Default, Serialize)]
struct Phase {
    phase: String,
    inputs: usize,
    targets: usize,
    failed_targets: usize,
    forecasts: usize,
    failed_forecasts: usize,
    max_forecast_error_ms: Option<i64>,
    forecast_lead_ms: BTreeMap<i64, usize>,
    unknown_lead_forecasts: usize,
    failure_counts: BTreeMap<String, usize>,
    unmatched_displays: usize,
    duplicate_candidates: usize,
}
impl Phase {
    fn observe(&mut self, boss: &Boss) {
        self.inputs += 1;
        self.targets += boss.targets.len();
        self.failed_targets += boss.targets.iter().filter(|target| !target.passed).count();
        self.unmatched_displays += boss.unmatched_displays.len();
        self.duplicate_candidates += boss
            .unmatched_displays
            .iter()
            .filter(|row| row.get("kind").and_then(Value::as_str) == Some("duplicate_candidate"))
            .count();
        for forecast in boss
            .targets
            .iter()
            .filter_map(|target| target.forecast.as_ref())
        {
            self.forecasts += 1;
            self.failed_forecasts += usize::from(!forecast.passed);
            self.max_forecast_error_ms = self.max_forecast_error_ms.max(forecast.error_ms);
            if let Some(lead) = forecast.lead_ms {
                *self.forecast_lead_ms.entry(lead).or_default() += 1;
            } else {
                self.unknown_lead_forecasts += 1;
            }
        }
        for target in boss.targets.iter().filter(|target| !target.passed) {
            if target.failure_reasons.is_empty() {
                *self
                    .failure_counts
                    .entry("legacy_untagged_failure".into())
                    .or_default() += 1;
            }
            for reason in &target.failure_reasons {
                *self.failure_counts.entry(reason.clone()).or_default() += 1;
            }
        }
    }
}
#[derive(Serialize)]
struct Unknown {
    phase: String,
    source: String,
    id: String,
    name: Option<String>,
    count: usize,
    inputs: usize,
    example_input: String,
    raw_index: usize,
}

fn add_audit(
    unknown: &mut BTreeMap<(String, String, String), Unknown>,
    phase: &str,
    input: &str,
    audit: &Audit,
) -> Result<()> {
    for row in &audit.unclassified {
        let group = unknown
            .entry((phase.to_owned(), row.source.clone(), row.id.clone()))
            .or_insert_with(|| Unknown {
                phase: phase.to_owned(),
                source: row.source.clone(),
                id: row.id.clone(),
                name: row.name.clone(),
                count: 0,
                inputs: 0,
                example_input: input.to_owned(),
                raw_index: row.first_raw_index,
            });
        group.count = group
            .count
            .checked_add(row.count)
            .context("Audit count overflow")?;
        group.inputs += 1;
    }
    Ok(())
}

fn brief_diagnostic(row: &Value) -> Value {
    json!({
        "at_ms":row.get("at_ms"),
        "action":row.get("action").and_then(Value::as_str).map(|text|text.chars().take(32).collect::<String>()),
        "detail":row.get("detail").and_then(Value::as_str).map(|text|text.chars().take(400).collect::<String>())
    })
}
fn forecast_diagnostic(forecast: &Forecast) -> Option<Value> {
    [
        ("current", &forecast.current_diagnostics),
        ("projection", &forecast.projection_diagnostics),
        ("legacy_combined", &forecast.diagnostics),
    ]
    .into_iter()
    .find_map(|(scope, rows)| {
        rows.first()
            .map(|row| json!({"scope":scope,"record":brief_diagnostic(row)}))
    })
}
fn first_issue(trace: &Trace) -> Option<Value> {
    if trace.status != "evaluated" {
        // Bound error text too, e.g. a large malformed JSON must not flood the authoring context.
        let error = trace
            .error
            .as_deref()
            .map(|error| error.chars().take(400).collect::<String>());
        let mut issue =
            json!({"input":trace.input,"status":trace.status,"stage":trace.stage,"error":error});
        if let Some((fields, safe_entry)) = issue.as_object_mut().zip(trace.safe_entry) {
            fields.insert("safe_entry".into(), json!(safe_entry));
        }
        if let Some((fields, candidates)) = issue.as_object_mut().zip(
            trace
                .evidence
                .as_ref()
                .and_then(|evidence| evidence.entry_candidates.as_ref()),
        ) {
            fields.insert("entry_candidate_count".into(), json!(candidates.len()));
            let examples = candidates.iter().take(3).map(|entry| {
                json!({"phase":entry.phase.chars().take(120).collect::<String>(),"raw_index":entry.raw_index})
            }).collect::<Vec<_>>();
            fields.insert("entry_candidates".into(), json!(examples));
        }
        if let Some(fields) = issue
            .as_object_mut()
            .filter(|_| !trace.unverified_phases.is_empty())
        {
            let phases = trace
                .unverified_phases
                .iter()
                .take(3)
                .map(|phase| phase.chars().take(120).collect::<String>())
                .collect::<Vec<_>>();
            fields.insert(
                "unverified_phase_count".into(),
                json!(trace.unverified_phases.len()),
            );
            fields.insert("unverified_phases".into(), json!(phases));
        }
        return Some(issue);
    }
    if let Some(mismatch) = &trace.entry_mismatch {
        return Some(json!({
            "input":trace.input,
            "safe_entry":trace.safe_entry,
            "entry_mismatch":mismatch,
            "diagnostics":trace.diagnostics.len(),
            "first_diagnostic":trace.diagnostics.first().map(brief_diagnostic),
        }));
    }
    let failed_targets = trace
        .bosses
        .iter()
        .flat_map(|boss| &boss.targets)
        .filter(|target| !target.passed)
        .count();
    if let Some((boss, target)) = trace.bosses.iter().find_map(|boss| {
        boss.targets
            .iter()
            .find(|target| !target.passed)
            .map(|target| (boss, target))
    }) {
        return Some(
            json!({"input":trace.input,"phase":boss.phase,"raw_index":target.raw_index,"at_ms":target.at_ms,"id":target.id,"failed_targets":failed_targets,"failure_reasons":target.failure_reasons,"replay_error_ms":target.error_ms,"forecast_error_ms":target.forecast.as_ref().and_then(|f|f.error_ms),"diagnostic":target.forecast.as_ref().and_then(forecast_diagnostic)}),
        );
    }
    if trace.passed != Some(true) {
        let first = trace.diagnostics.first().map(brief_diagnostic);
        return Some(
            json!({"input":trace.input,"safe_entry":trace.safe_entry,"diagnostics":trace.diagnostics.len(),"first_diagnostic":first}),
        );
    }
    if let Some((boss, display)) = trace.bosses.iter().find_map(|boss| {
        boss.unmatched_displays
            .first()
            .map(|display| (boss, display))
    }) {
        return Some(
            json!({"input":trace.input,"phase":boss.phase,"kind":"unmatched_display","unmatched_displays":boss.unmatched_displays.len(),"event_id":display.pointer("/display/event/id").and_then(Value::as_str).map(|id|id.chars().take(120).collect::<String>()),"at_ms":display.pointer("/display/at_ms")}),
        );
    }
    None
}

#[derive(Default)]
struct OccurrenceCoverage<'a> {
    observed: usize,
    missing: usize,
    example: Option<(&'a str, &'a Target)>,
}

fn missing_occurrences(traces: &[Trace], limit: usize) -> (Vec<Value>, usize) {
    let mut coverage = BTreeMap::<(&str, &str, usize), OccurrenceCoverage<'_>>::new();
    for (trace, boss) in traces
        .iter()
        .flat_map(|trace| trace.bosses.iter().map(move |boss| (trace, boss)))
    {
        let mut ordinals = BTreeMap::<&str, usize>::new();
        let mut targets = boss.targets.iter().collect::<Vec<_>>();
        targets.sort_by_key(|target| (target.at_ms, target.raw_index));
        for target in targets {
            let ordinal = ordinals.entry(&target.id).or_default();
            *ordinal += 1;
            let row = coverage
                .entry((&boss.phase, &target.id, *ordinal))
                .or_default();
            row.observed += 1;
            // An absent observation cannot establish a missed display.
            if target
                .failure_reasons
                .iter()
                .any(|reason| reason == "missing_display")
            {
                row.missing += 1;
                row.example.get_or_insert((&trace.input, target));
            }
        }
    }
    let mut gaps = coverage
        .into_iter()
        .filter(|(_, row)| row.missing > 0)
        .collect::<Vec<_>>();
    gaps.sort_by_key(|(key, row)| (std::cmp::Reverse(row.missing), *key));
    let omitted = gaps.len().saturating_sub(limit);
    let examples = gaps.into_iter().take(limit).filter_map(|((phase, id, ordinal), row)| {
        let (input, target) = row.example?;
        Some(json!({"phase":phase,"id":id,"occurrence":ordinal,"observed":row.observed,"missing":row.missing,"example":{"input":input,"raw_index":target.raw_index,"at_ms":target.at_ms}}))
    }).collect();
    (examples, omitted)
}

fn validate_summary(report: &Report) -> Result<()> {
    ensure!(
        report.version.is_none_or(|v| v == 1) && report.summary.is_object(),
        "Unsupported evaluation report"
    );
    let statuses = report
        .traces
        .iter()
        .map(|trace| json!({"status":trace.status,"passed":trace.passed}))
        .collect::<Vec<_>>();
    ensure!(
        report.summary == super::summary(&statuses),
        "Report summary disagrees with traces"
    );
    Ok(())
}

fn compact(report: &Report, limit: usize, offset: usize) -> Result<Value> {
    validate_summary(report)?;
    let mut phases = BTreeMap::<String, Phase>::new();
    let mut unknown = BTreeMap::<(String, String, String), Unknown>::new();
    let mut audited = 0;
    let mut scopes = 0;
    for trace in &report.traces {
        ensure!(
            ["evaluated", "excluded", "error", "unverified"].contains(&trace.status.as_str()),
            "Invalid trace status"
        );
        for boss in &trace.bosses {
            scopes += 1;
            let phase = phases.entry(boss.phase.clone()).or_insert_with(|| Phase {
                phase: boss.phase.clone(),
                ..Phase::default()
            });
            phase.observe(boss);
            let Some(audit) = &boss.audit else {
                continue;
            };
            audited += 1;
            add_audit(&mut unknown, &boss.phase, &trace.input, audit)?;
        }
        for scope in trace
            .evidence
            .iter()
            .flat_map(|evidence| &evidence.audit_only)
        {
            scopes += 1;
            audited += 1;
            add_audit(&mut unknown, &scope.phase, &trace.input, &scope.audit)?;
        }
    }
    let mut issue_traces = report
        .traces
        .iter()
        .filter(|trace| {
            trace.status != "evaluated"
                || trace.passed != Some(true)
                || trace
                    .bosses
                    .iter()
                    .any(|boss| !boss.unmatched_displays.is_empty())
        })
        .collect::<Vec<_>>();
    // Show failures before review warnings and exclusions, e.g. an unmatched tail must not hide a tool error.
    issue_traces.sort_by_key(|trace| (trace.status == "excluded", trace.passed == Some(true)));
    let issues = issue_traces
        .into_iter()
        .filter_map(first_issue)
        .collect::<Vec<_>>();
    let unaudited_inputs = report
        .traces
        .iter()
        .filter(|trace| {
            trace.bosses.is_empty()
                && trace
                    .evidence
                    .as_ref()
                    .is_none_or(|evidence| evidence.audit_only.is_empty())
        })
        .count();
    let audit_status = if audited != scopes || scopes == 0 || unaudited_inputs > 0 {
        "unavailable_or_partial"
    } else if unknown.is_empty() {
        "classified"
    } else {
        "needs_review"
    };
    let mut phase_rows = phases.values().collect::<Vec<_>>();
    phase_rows.sort_by_key(|phase| (std::cmp::Reverse(phase.failed_targets), &phase.phase));
    ensure!(offset <= unknown.len(), "Unclassified offset out of bounds");
    let unknown_rows = unknown
        .values()
        .skip(offset)
        .take(limit)
        .collect::<Vec<_>>();
    let next = offset + unknown_rows.len();
    let (missing, omitted_missing) = missing_occurrences(&report.traces, limit);
    Ok(json!({
        "summary": report.summary, "profile_sha256": report.profile_sha256, "evaluator_sha256": report.evaluator_sha256, "bindings": report.bindings,
        "phases": phase_rows.iter().take(limit).collect::<Vec<_>>(), "omitted_phases": phases.len().saturating_sub(limit),
        "issues": issues.iter().take(limit).collect::<Vec<_>>(), "issue_traces": issues.len(), "omitted_issues": issues.len().saturating_sub(limit),
        "missing_occurrences": missing, "omitted_missing_occurrences": omitted_missing, "audit_status": audit_status,
        "unaudited_inputs": unaudited_inputs, "unclassified_groups": unknown.len(),
        "unclassified": unknown_rows, "omitted_unclassified": unknown.len() - unknown_rows.len(),
        "unclassified_offset": offset, "next_unclassified_offset": (next < unknown.len()).then_some(next),
        "scope": "declared evaluation sources and intervals; other helpers and out-of-scope activity are not audited",
        "coverage":coverage::summarize(&report.traces,report.evidence_role.unwrap_or_default(),limit)?
    }))
}
impl Run for ReviewCommand {
    type Output = Result<()>;

    fn run(self) -> Result<()> {
        ensure!(
            (1..=50).contains(&self.limit),
            "Review limit must be in 1..50"
        );
        let bytes = fs::read(&self.report)?;
        let report: Report = serde_json::from_slice(&bytes).context("Reading evaluation report")?;
        let result = compact(&report, self.limit, self.unclassified_offset)?;
        println!(
            "{}",
            json!({"report":self.report.as_str(),"sha256":super::hash(&bytes),"review":result})
        );
        Ok(())
    }
}
