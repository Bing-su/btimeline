use std::collections::BTreeSet;

use anyhow::{Result, ensure};
use serde_json::{Value, json};

use super::Manifest;
use super::projection::{Projection, Segment, Target};
use crate::timeline::{Record, Recording, Timeline};

pub(super) struct Support<'a> {
    pub record: &'a Record,
    pub raw_index: usize,
    pub observed_at_ms: i64,
}
struct Expected<'a> {
    phase: &'a str,
    scope: Option<(usize, usize)>,
    execution: Option<&'a Record>,
}
fn diagnostic(record: &Record) -> bool {
    matches!(
        record.action.as_str(),
        "ambiguous" | "unresolved" | "violation"
    )
}
fn event_view(row: &Record) -> Value {
    json!({"event":row.event,"context":row.context,"run":row.run,"at_ms":row.at_ms})
}

fn forecast_target(
    model: &Timeline,
    recording: &Recording,
    target: &Target,
    expected: &Expected<'_>,
    lead: i64,
    manifest: &Manifest,
) -> Result<Value> {
    let result = crate::timeline::forecast(
        model,
        recording,
        target.at_ms - lead,
        manifest.horizon_ms,
        10000,
    )?;
    ensure!(
        !result.truncated,
        "Forecast truncated: {:?}",
        result.truncation_reason
    );
    let executed_id = expected
        .execution
        .and_then(|row| row.event.as_ref())
        .map(|event| event.id.as_str());
    // Replacement at a merge is equivalent only when the author explicitly declares that handoff.
    let ids = executed_id
        .map(|id| {
            target
                .equivalents
                .iter()
                .find(|set| set.iter().any(|event| event == id))
                .map_or_else(|| vec![id], |set| set.iter().map(String::as_str).collect())
        })
        .unwrap_or_else(|| target.event_ids.iter().map(String::as_str).collect());
    let executed_context = expected.execution.and_then(|row| row.context.as_ref());
    // Conditional alternatives are display candidates, never successful committed forecasts.
    let matched = result
        .events
        .iter()
        .filter(|event| {
            ids.contains(&event.id.as_str())
                && event.context.owner.phase.as_deref() == Some(expected.phase)
                && expected.scope == Some((event.run, event.context.phase_instance))
                && executed_context.is_none_or(|actual| {
                    actual.owner.loop_id == event.context.owner.loop_id
                        && actual.iteration == event.context.iteration
                        && actual.owner.sequence == event.context.owner.sequence
                        && actual.sequence_instance == event.context.sequence_instance
                })
        })
        .min_by_key(|event| target.timing_error(event.at_ms, event.duration_ms));
    let error = matched.map(|event| target.timing_error(event.at_ms, event.duration_ms));
    let passed = result.phase.as_deref() == Some(expected.phase)
        && result.status == "active"
        && error.is_some_and(|error| error <= manifest.tolerance_ms);
    let diagnostics = result
        .diagnostics
        .iter()
        .chain(&result.projection_diagnostics)
        .collect::<Vec<_>>();
    Ok(
        json!({"error_ms":error,"passed":passed,"diagnostics":diagnostics,"current_diagnostics":result.diagnostics,"projection_diagnostics":result.projection_diagnostics,"phase":result.phase,"status":result.status,"lead_ms":lead,"matched_event":matched}),
    )
}

fn failure_reasons(error: Option<i64>, forecast: &Value, tolerance: i64) -> Vec<&'static str> {
    let mut reasons = Vec::new();
    match error {
        None => reasons.push("missing_display"),
        Some(error) if error > tolerance => reasons.push("display_timing"),
        _ => {}
    }
    if forecast.is_null() {
        return reasons;
    }
    if forecast.get("passed") == Some(&Value::Bool(false)) {
        reasons.push(match forecast.get("error_ms").and_then(Value::as_i64) {
            None => "forecast_missing",
            Some(error) if error > tolerance => "forecast_timing",
            _ => "forecast_control_state",
        });
    }
    for (field, reason) in [
        ("current_diagnostics", "current_diagnostic"),
        ("projection_diagnostics", "projection_diagnostic"),
    ] {
        if forecast
            .get(field)
            .and_then(Value::as_array)
            .is_some_and(|rows| !rows.is_empty())
        {
            reasons.push(reason);
        }
    }
    reasons
}
fn target_result(
    model: &Timeline,
    projected: &Projection,
    target: &Target,
    expected: &Expected<'_>,
    entry_ms: i64,
    manifest: &Manifest,
) -> Result<Value> {
    let error = expected.execution.map(|row| {
        target.timing_error(
            row.at_ms,
            row.event.as_ref().map_or(0, |event| event.duration_ms),
        )
    });
    let lead = target.lead_ms.unwrap_or(manifest.forecast_lead_ms);
    let forecast = if target.at_ms - entry_ms >= lead {
        forecast_target(
            model,
            &projected.recording,
            target,
            expected,
            lead,
            manifest,
        )?
    } else {
        Value::Null
    };
    let passed = error.is_some_and(|error| error <= manifest.tolerance_ms)
        && (forecast.is_null()
            || (forecast.get("passed") == Some(&Value::Bool(true))
                && forecast
                    .get("diagnostics")
                    .and_then(Value::as_array)
                    .is_some_and(Vec::is_empty)));
    Ok(
        json!({"raw_index":target.raw_index,"raw_indices":target.raw_indices,"at_ms":target.at_ms,"id":target.id,"name":target.name,"reference":target.reference,"event_ids":target.event_ids,"error_ms":error,"failure_reasons":failure_reasons(error,&forecast,manifest.tolerance_ms),"forecast":forecast,"passed":passed,"matched_event":expected.execution.map(event_view)}),
    )
}

fn segment_score<'a>(
    model: &Timeline,
    projected: &Projection,
    boss: &Segment,
    rows: &'a [Record],
    manifest: &Manifest,
) -> Result<(Value, Vec<Support<'a>>)> {
    let scope = rows
        .iter()
        .find(|row| {
            row.action == "phase"
                && row.at_ms == boss.entry_ms
                && row
                    .context
                    .as_ref()
                    .is_some_and(|context| context.owner.phase.as_deref() == Some(&boss.phase))
        })
        .and_then(|row| {
            row.context
                .as_ref()
                .map(|context| (row.run, context.phase_instance))
        });
    let events = rows
        .iter()
        .filter(|row| {
            row.action == "event"
                && row.event.as_ref().is_some_and(|event| event.visible)
                && row.at_ms >= boss.entry_ms
                && row.at_ms <= boss.end_ms
                && row.context.as_ref().is_some_and(|context| {
                    context.owner.phase.as_deref() == Some(&boss.phase)
                        && scope == Some((row.run, context.phase_instance))
                })
        })
        .collect::<Vec<_>>();
    let assignment = super::matching::assign(&boss.targets, &events, manifest.tolerance_ms)?;
    let mut used = BTreeSet::new();
    let mut support = Vec::new();
    let targets = boss
        .targets
        .iter()
        .zip(&assignment)
        .map(|(target, index)| {
            let execution = index.and_then(|index| events.get(index).copied());
            if let Some(index) = index {
                used.insert(*index);
            }
            if let Some(record) = execution.filter(|row| {
                target.timing_error(
                    row.at_ms,
                    row.event.as_ref().map_or(0, |event| event.duration_ms),
                ) <= manifest.tolerance_ms
            }) {
                support.push(Support {
                    record,
                    raw_index: target.raw_index,
                    observed_at_ms: target.at_ms,
                });
            }
            target_result(
                model,
                projected,
                target,
                &Expected {
                    phase: &boss.phase,
                    scope,
                    execution,
                },
                boss.entry_ms,
                manifest,
            )
        })
        .collect::<Result<Vec<_>>>()?;
    let unmatched = events.iter().enumerate().filter(|(index,_)|!used.contains(index)).map(|(_,row)| {
        let id = row.event.as_ref().map(|event|event.id.as_str()).expect("structured event");
        let bound = boss.targets.iter().any(|target|target.event_ids.iter().any(|event|event==id));
        let near_target = boss.targets.iter().any(|target|target.event_ids.iter().any(|event|event==id) && row.at_ms.abs_diff(target.at_ms)<=manifest.tolerance_ms.unsigned_abs());
        json!({"kind":if near_target {"duplicate_candidate"}else if bound {"unmatched_bound"}else{"unbound"},"display":event_view(row)})
    }).collect::<Vec<_>>();
    Ok((
        json!({"phase":boss.phase,"entry_ms":boss.entry_ms,"end_ms":boss.end_ms,"entry_event":boss.entry_event,"completion_event":boss.completion_event,"targets":targets,"audit":boss.audit,"unmatched_displays":unmatched}),
        support,
    ))
}

// The matcher consumes each emitted occurrence once; future queries never read observations past their cutoff.
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
    ensure!(
        rows.iter()
            .filter(|row| row.action == "event")
            .all(|row| row.event.is_some() && row.context.is_some()),
        "Replay lacks structured event identity"
    );
    let diagnostics = rows
        .iter()
        .filter(|row| diagnostic(row))
        .collect::<Vec<_>>();
    let entry_mismatch = first_entry_mismatch(&projected.bosses, &rows);
    let safe_entry = entry_mismatch.is_none();
    let evaluated = projected
        .bosses
        .iter()
        .map(|boss| segment_score(model, projected, boss, &rows, manifest))
        .collect::<Result<Vec<_>>>()?;
    let support = evaluated
        .iter()
        .flat_map(|(_, support)| support)
        .collect::<Vec<_>>();
    let phases = projected
        .bosses
        .iter()
        .map(|boss| boss.phase.as_str())
        .collect();
    let coverage = super::coverage::collect(model, &rows, &support, &phases);
    let bosses = evaluated.iter().map(|(boss, _)| boss).collect::<Vec<_>>();
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
    let review_required = projected
        .bosses
        .iter()
        .any(|boss| boss.audit.unclassified_casts > 0)
        || bosses.iter().any(|boss| {
            boss.get("unmatched_displays")
                .and_then(Value::as_array)
                .is_some_and(|rows| !rows.is_empty())
        });
    Ok(
        json!({"origin_timestamp_ms":projected.origin_timestamp_ms,"cast_observations":projected.recording.observations.len(),"safe_entry":safe_entry,"entry_mismatch":entry_mismatch,"diagnostics":diagnostics,"bosses":bosses,"passed":passed,"review_required":review_required,"matched_occurrences":support.len(),"coverage":coverage}),
    )
}

fn first_entry_mismatch(bosses: &[Segment], rows: &[Record]) -> Option<Value> {
    let mut expected = bosses.iter();
    let mut replayed = rows.iter().filter(|row| {
        row.action == "phase"
            && bosses
                .iter()
                .any(|boss| row.at_ms >= boss.entry_ms && row.at_ms <= boss.end_ms)
    });
    for index in 0.. {
        let (boss, row) = (expected.next(), replayed.next());
        if boss.is_none() && row.is_none() {
            break;
        }
        let phase = row.and_then(|row| row.context.as_ref()?.owner.phase.as_deref());
        if boss.map(|boss| (boss.entry_ms, Some(boss.phase.as_str())))
            != row.map(|row| (row.at_ms, phase))
        {
            // A missed entry can invalidate the forecasts for every later phase.
            return Some(json!({
                "index": index,
                "expected": boss.map(|boss| json!({"phase":boss.phase,"at_ms":boss.entry_ms,"raw_index":boss.entry_event})),
                "replayed": row.map(|row| json!({"phase":phase,"at_ms":row.at_ms,"raw_index":row.source_event_index})),
            }));
        }
    }
    None
}
