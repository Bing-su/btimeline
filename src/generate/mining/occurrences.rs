use std::collections::BTreeMap;

use anyhow::{Context, Result};
use serde_json::{Value, json};

use super::{Pull, Signal, alignment};

pub(in crate::generate) struct Event {
    pub signal: Signal,
    pub samples: BTreeMap<String, i64>,
    pub occurrence: Option<usize>,
    pub evidence: Value,
}

pub(in crate::generate) struct Common {
    pub events: Vec<Event>,
    pub diagnostics: Vec<Value>,
}

struct Observation<'a> {
    signal: &'a Signal,
    batch: usize,
    at: i64,
    ordinal: usize,
}

fn trace(pull: &Pull) -> String {
    format!("{}/{}", pull.report, pull.fight)
}

// shortcut: defer unordered intervals until set-aware alignment exists; simultaneous A,B has no order.
fn interval<'a>(
    pull: &'a Pull,
    left: i64,
    right: i64,
    source: &Option<String>,
    include: &impl Fn(&Signal) -> bool,
) -> Result<Vec<Observation<'a>>, &'static str> {
    if pull
        .phase_transitions
        .iter()
        .any(|phase| left < phase.relative_ms && phase.relative_ms < right)
    {
        return Err("phase boundary");
    }
    let mut counts = BTreeMap::new();
    let mut result = Vec::new();
    let mut previous = None;
    for (batch, frame) in pull.batches.iter().enumerate() {
        let selected: Vec<_> = frame
            .signals
            .iter()
            .filter(|signal| &signal.source == source && include(signal))
            .collect();
        for signal in &selected {
            *counts.entry(*signal).or_insert(0) += 1;
        }
        if frame.at_ms < left || frame.at_ms > right || selected.is_empty() {
            continue;
        }
        if selected.len() != 1
            || previous.is_some_and(|at| frame.at_ms - at <= super::common::ORDER_RESOLUTION_MS)
        {
            return Err("unordered observations");
        }
        previous = Some(frame.at_ms);
        if left < frame.at_ms && frame.at_ms < right {
            let signal = selected.first().ok_or("missing observation")?;
            result.push(Observation {
                signal,
                batch,
                at: frame.at_ms,
                ordinal: *counts.get(signal).ok_or("missing ordinal")?,
            });
        }
    }
    Ok(result)
}

fn unrepresented(pull: &Pull, events: &[Event], include: &impl Fn(&Signal) -> bool) -> Value {
    let id = trace(pull);
    let rows: Vec<_> = pull
        .batches
        .iter()
        .enumerate()
        .flat_map(|(batch, frame)| {
            frame
                .signals
                .iter()
                .filter(|signal| include(signal))
                .filter(|signal| {
                    !events.iter().any(|event| {
                        &event.signal == *signal && event.samples.get(&id) == Some(&frame.at_ms)
                    })
                })
                .map(move |signal| json!({"batch":batch,"activity":signal}))
        })
        .collect();
    json!({"trace":id,"unrepresented":rows,"reason":"no forced bounded correspondence"})
}

fn pair(reference: &[Observation<'_>], peer: &[Observation<'_>]) -> Result<alignment::Alignment> {
    alignment::align(
        &reference.iter().map(|row| row.signal).collect::<Vec<_>>(),
        &peer.iter().map(|row| row.signal).collect::<Vec<_>>(),
    )
}

fn expand_interval(
    pulls: &[&Pull],
    rows: &[Vec<Observation<'_>>],
    bounds: Value,
    result: &mut Common,
) -> Result<()> {
    let reference = rows.first().context("Missing reference interval")?;
    if rows
        .iter()
        .any(|row| !alignment::within_limit(reference.len(), row.len()))
    {
        result
            .diagnostics
            .push(json!({"bounds":bounds,"status":"resource-limit"}));
        return Ok(());
    }
    let matches = rows
        .iter()
        .map(|row| pair(reference, row))
        .collect::<Result<Vec<_>>>()?;
    for ((pull, row), aligned) in pulls.iter().zip(rows).zip(&matches) {
        let batches = |indices: &[usize]| {
            indices
                .iter()
                .filter_map(|i| row.get(*i).map(|o| o.batch))
                .collect::<Vec<_>>()
        };
        result.diagnostics.push(json!({"bounds":bounds,"reference":trace(pulls.first().context("Missing reference")?),"trace":trace(pull),"optimal_matches":aligned.optimal_matches,
            "ambiguous_reference":aligned.ambiguous_left.iter().filter_map(|i|reference.get(*i).map(|o|o.batch)).collect::<Vec<_>>(),
            "reference_only":aligned.reference_only.iter().filter_map(|i|reference.get(*i).map(|o|o.batch)).collect::<Vec<_>>(),
            "ambiguous_peer":batches(&aligned.ambiguous_right),"peer_only":batches(&aligned.peer_only)}));
    }
    for (index, observation) in reference.iter().enumerate() {
        let peers = matches
            .iter()
            .map(|aligned| {
                aligned
                    .pairs
                    .iter()
                    .find(|(i, _)| *i == index)
                    .map(|(_, j)| *j)
            })
            .collect::<Option<Vec<_>>>();
        let Some(peers) = peers else { continue };
        let observations = rows
            .iter()
            .zip(peers)
            .map(|(row, j)| row.get(j).context("Missing aligned observation"))
            .collect::<Result<Vec<_>>>()?;
        let samples = pulls
            .iter()
            .zip(&observations)
            .map(|(pull, row)| (trace(pull), row.at))
            .collect();
        let batches: BTreeMap<_, _> = pulls
            .iter()
            .zip(&observations)
            .map(|(pull, row)| (trace(pull), row.batch))
            .collect();
        result.events.push(Event {signal:observation.signal.clone(),samples,occurrence:Some(observation.ordinal),
            evidence:json!({"method":"anchor-constrained-lcs-v1","bounds":bounds,"reference":trace(pulls.first().context("Missing reference")?),"batches":batches,"sync":"omitted: repeated signal identity"})});
    }
    Ok(())
}

pub(super) fn expand(
    pulls: &[Pull],
    unique: BTreeMap<Signal, BTreeMap<String, i64>>,
    include: impl Fn(&Signal) -> bool,
) -> Result<Common> {
    let mut completed: Vec<_> = pulls.iter().filter(|pull| pull.kill).collect();
    // The longest completed trace supplies readable occurrence labels; ties ignore CLI argument order.
    completed.sort_by_key(|pull| {
        (
            std::cmp::Reverse(
                pull.batches
                    .iter()
                    .flat_map(|b| &b.signals)
                    .filter(|s| include(s))
                    .count(),
            ),
            trace(pull),
        )
    });
    let reference = trace(completed.first().context("Missing reference")?);
    let mut result = Common {
        events: unique
            .iter()
            .map(|(signal, samples)| Event {
                signal: signal.clone(),
                samples: samples.clone(),
                occurrence: None,
                evidence: Value::Null,
            })
            .collect(),
        diagnostics: Vec::new(),
    };
    let mut scopes = BTreeMap::<_, Vec<_>>::new();
    for (signal, samples) in &unique {
        scopes
            .entry(&signal.source)
            .or_default()
            .push((signal, samples));
    }
    for (source, mut anchors) in scopes {
        anchors.sort_by_key(|(_, samples)| samples.get(&reference));
        for window in anchors.windows(2) {
            let [(left, a), (right, b)] = window else {
                continue;
            };
            let bounds = json!({"left":left,"right":right});
            let rows = completed
                .iter()
                .map(|pull| {
                    let id = trace(pull);
                    let start = a.get(&id).ok_or((id.clone(), "missing bound"))?;
                    let end = b.get(&id).ok_or((id.clone(), "missing bound"))?;
                    interval(pull, *start, *end, source, &include).map_err(|reason| (id, reason))
                })
                .collect::<Result<Vec<_>, _>>();
            match rows {
                Ok(rows) => expand_interval(&completed, &rows, bounds, &mut result)?,
                Err((trace, reason)) => result.diagnostics.push(
                    json!({"bounds":bounds,"trace":trace,"status":"unresolved","reason":reason}),
                ),
            }
        }
    }
    // A represented B-02 must not hide B-01 outside the bounds or in an ambiguous interval.
    result.diagnostics.extend(
        completed
            .iter()
            .map(|pull| unrepresented(pull, &result.events, &include)),
    );
    Ok(result)
}
