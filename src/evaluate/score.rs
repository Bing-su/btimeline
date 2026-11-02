use anyhow::{Result, ensure};
use serde_json::{Value, json};

use super::Manifest;
use super::projection::{Projection, Target};
use crate::timeline::{Record, Recording, Timeline};

fn diagnostic(record: &Record) -> bool {
    matches!(
        record.action.as_str(),
        "ambiguous" | "unresolved" | "violation"
    )
}
fn forecast_target(
    model: &Timeline,
    recording: &Recording,
    target: &Target,
    phase: &str,
    lead: i64,
    manifest: &Manifest,
) -> Result<Value> {
    let result =
        crate::timeline::forecast(model, recording, target.at_ms - lead, manifest.horizon_ms)?;
    let error = result
        .events
        .iter()
        .filter(|event| event.name == target.name)
        .map(|event| (event.at_ms - target.at_ms).abs())
        .min();
    let passed = result.phase.as_deref() == Some(phase)
        && result.status == "active"
        && error.is_some_and(|error| error <= manifest.tolerance_ms);
    let mut diagnostics = result.diagnostics;
    diagnostics.extend(result.projection_diagnostics);
    let mut forecast = json!({"error_ms":error,"passed":passed,"diagnostics":diagnostics});
    if target.lead_ms.is_some() {
        forecast
            .as_object_mut()
            .expect("forecast object")
            .insert("lead_ms".into(), json!(lead));
    }
    Ok(forecast)
}

// Evaluate forecasts directly in process; no temporary recording, CLI spawning or repeated KDL parsing.
pub(super) fn score(
    model: &Timeline,
    projected: &Projection,
    manifest: &Manifest,
) -> Result<Value> {
    let rows = crate::timeline::replay(model, &projected.recording)?;
    ensure!(
        rows.iter().any(|row| row.action == "start"),
        "Projected recording did not start the timeline"
    );
    let diagnostics = rows
        .iter()
        .filter(|row| diagnostic(row))
        .collect::<Vec<_>>();
    let transitions = rows
        .iter()
        .filter(|row| row.action == "phase")
        .map(|row| (row.at_ms, row.detail.as_str()))
        .collect::<Vec<_>>();
    let expected = projected
        .bosses
        .iter()
        .map(|boss| (boss.entry_ms, boss.phase.as_str()))
        .collect::<Vec<_>>();
    let safe_entry = transitions == expected;
    let bosses = projected.bosses.iter().map(|boss| {
        let targets = boss.targets.iter().map(|target| {
            // Preserve the previous acceptance rule during migration; structured event matching is separate work.
            let label = format!(": {} (visible=true,", target.name);
            let error = rows.iter()
                .filter(|row| row.action == "event"
                    && row.at_ms >= boss.entry_ms && row.at_ms <= boss.end_ms
                    && row.detail.contains(&label))
                .map(|row| (row.at_ms - target.at_ms).abs())
                .min();
            let lead = target.lead_ms.unwrap_or(manifest.forecast_lead_ms);
            let forecast = if target.at_ms - boss.entry_ms >= lead {
                forecast_target(model, &projected.recording, target, &boss.phase, lead, manifest)?
            } else { Value::Null };
            let forecast_passed = forecast.is_null()
                || (forecast.get("passed") == Some(&Value::Bool(true))
                    && forecast.get("diagnostics").and_then(Value::as_array).is_some_and(Vec::is_empty));
            let passed = error.is_some_and(|error| error <= manifest.tolerance_ms) && forecast_passed;
            Ok(json!({
                "raw_index": target.raw_index, "at_ms": target.at_ms, "id": target.id,
                "error_ms": error, "forecast": forecast, "passed": passed
            }))
        }).collect::<Result<Vec<_>>>()?;
        Ok(json!({
            "phase": boss.phase, "entry_ms": boss.entry_ms, "end_ms": boss.end_ms,
            "entry_event": boss.entry_event, "completion_event": boss.completion_event, "targets": targets
        }))
    }).collect::<Result<Vec<_>>>()?;
    let passed = safe_entry
        && diagnostics.is_empty()
        && bosses.iter().all(|boss| {
            boss.get("targets")
                .and_then(Value::as_array)
                .is_some_and(|targets| {
                    targets
                        .iter()
                        .all(|target| target.get("passed") == Some(&Value::Bool(true)))
                })
        });
    Ok(
        json!({"origin_timestamp_ms":projected.origin_timestamp_ms,"cast_observations":projected.recording.observations.len(),"safe_entry":safe_entry,"diagnostics":diagnostics,"bosses":bosses,"passed":passed}),
    )
}
