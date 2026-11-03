//! Signal-supported choices and matched loop anchors are separate from timer-only execution and synthetic inputs.
use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::score::Support;
use crate::timeline::{LoopDefinition, Owner, Record, Timeline};
mod body;
mod boundary;
pub(super) use body::BodyCoverage;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum EvidenceRole {
    Development,
    HeldOut,
    Synthetic,
    #[default]
    Unspecified,
}
#[derive(Deserialize, Serialize)]
pub(super) struct Selection {
    pub raw_index: usize,
    pub at_ms: i64,
    pub run: usize,
    pub phase_instance: usize,
    pub iteration: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resume_at_ms: Option<i64>,
}
#[derive(Deserialize, Serialize)]
pub(super) struct OptionCoverage {
    pub id: String,
    pub fallback: Option<bool>,
    #[serde(default)]
    pub event_ids: BTreeSet<String>,
    pub selections: Vec<Selection>,
}
#[derive(Deserialize, Serialize)]
pub(super) struct BranchCoverage {
    pub id: String,
    pub owner: Owner,
    pub options: Vec<OptionCoverage>,
    #[serde(default)]
    pub unresolved: Vec<UnresolvedBranch>,
}
#[derive(Deserialize, Serialize)]
pub(super) struct UnresolvedBranch {
    at_ms: i64,
    raw_index: Option<usize>,
}
#[derive(Deserialize, Serialize)]
pub(super) struct AnchorPair {
    pub event_id: String,
    pub run: usize,
    pub phase_instance: usize,
    pub iteration: u64,
    pub from_raw_index: usize,
    pub to_raw_index: usize,
}
#[derive(Deserialize, Serialize)]
pub(super) struct LoopCoverage {
    pub id: String,
    pub owner: Owner,
    pub emitted_iterations: usize,
    pub matched_iterations: usize,
    pub matched_events: usize,
    pub anchor_recurrences: Vec<AnchorPair>,
    pub signal_exits: usize,
    pub body: Option<BodyCoverage>,
    pub outside_body_candidate: Option<boundary::Candidate>,
}
#[derive(Deserialize, Serialize)]
pub(super) struct Coverage {
    pub branches: Vec<BranchCoverage>,
    pub loops: Vec<LoopCoverage>,
}

pub(super) fn collect(
    model: &Timeline,
    rows: &[Record],
    support: &[&Support<'_>],
    phases: &BTreeSet<&str>,
) -> Coverage {
    let branches = model
        .catalog
        .branches
        .iter()
        .filter(|branch| {
            branch
                .owner
                .phase
                .as_deref()
                .is_some_and(|phase| phases.contains(phase))
        })
        .map(|branch| {
            let options = branch
                .options
                .iter()
                .map(|option| OptionCoverage {
                    id: option.clone(),
                    fallback: Some(branch.fallback_options.contains(option)),
                    event_ids: model
                        .catalog
                        .events
                        .values()
                        .filter(|event| {
                            event.event.visible
                                && event.owner.phase == branch.owner.phase
                                && event.owner.branch.as_deref() == Some(&branch.id)
                                && event.owner.option.as_deref() == Some(option)
                        })
                        .map(|event| event.event.id.clone())
                        .collect(),
                    selections: rows
                        .iter()
                        .filter(|row| row.action == "option")
                        .filter_map(|row| {
                            let context = row.context.as_ref()?;
                            let raw_index = row.source_event_index?;
                            (context.owner.phase == branch.owner.phase
                                && context.owner.branch.as_deref() == Some(&branch.id)
                                && context.owner.option.as_deref() == Some(option))
                            .then_some(Selection {
                                raw_index,
                                at_ms: row.at_ms,
                                run: row.run,
                                phase_instance: context.phase_instance,
                                iteration: context.iteration,
                                resume_at_ms: row.resume_at_ms,
                            })
                        })
                        .collect(),
                })
                .collect();
            BranchCoverage {
                id: branch.id.clone(),
                owner: branch.owner.clone(),
                options,
                unresolved: rows
                    .iter()
                    .filter(|row| row.unresolved_branch && row.detail == branch.id)
                    .map(|row| UnresolvedBranch {
                        at_ms: row.at_ms,
                        raw_index: row.source_event_index,
                    })
                    .collect(),
            }
        })
        .collect();
    let loops = model
        .catalog
        .loops
        .iter()
        .filter(|repeat| {
            repeat
                .owner
                .phase
                .as_deref()
                .is_some_and(|phase| phases.contains(phase))
        })
        .map(|repeat| loop_coverage(model, repeat, rows, support))
        .collect();
    Coverage { branches, loops }
}

fn loop_coverage(
    model: &Timeline,
    repeat: &LoopDefinition,
    rows: &[Record],
    support: &[&Support<'_>],
) -> LoopCoverage {
    let body = |row: &Record| {
        row.context
            .as_ref()
            .is_some_and(|context| context.owner.loop_id.as_deref() == Some(&repeat.id))
    };
    let key = |row: &Record| {
        row.context.as_ref().and_then(|context| {
            context
                .iteration
                .map(|iteration| (row.run, context.phase_instance, iteration))
        })
    };
    let emitted = rows
        .iter()
        .filter(|row| body(row))
        .filter_map(key)
        .collect::<BTreeSet<_>>();
    let matched = support
        .iter()
        .filter(|sample| body(sample.record))
        .collect::<Vec<_>>();
    let iterations = matched
        .iter()
        .filter_map(|sample| key(sample.record))
        .collect::<BTreeSet<_>>();
    let mut anchors = BTreeMap::new();
    for sample in &matched {
        if let Some((key, event)) = key(sample.record)
            .zip(sample.record.event.as_ref())
            .filter(|(_, event)| repeat.anchor_events.contains(&event.id))
        {
            anchors.insert((key.0, key.1, event.id.clone(), key.2), sample.raw_index);
        }
    }
    let mut pairs = BTreeMap::new();
    for ((run, phase, event, iteration), from) in &anchors {
        if let Some(to) = anchors.get(&(*run, *phase, event.clone(), iteration + 1)) {
            pairs
                .entry((*run, *phase, *iteration))
                .or_insert_with(|| AnchorPair {
                    event_id: event.clone(),
                    run: *run,
                    phase_instance: *phase,
                    iteration: *iteration,
                    from_raw_index: *from,
                    to_raw_index: *to,
                });
        }
    }
    let signal_exits = rows
        .iter()
        .filter(|row| {
            row.action == "until"
                && row.source_event_index.is_some()
                && row
                    .context
                    .as_ref()
                    .is_some_and(|context| context.owner.loop_id.as_deref() == Some(&repeat.id))
        })
        .count();
    let anchor_recurrences = pairs.into_values().collect::<Vec<_>>();
    let outside_body_candidate =
        boundary::first_candidate(repeat, rows, support, &anchor_recurrences);
    let body = body::collect(
        model,
        &repeat.id,
        rows,
        support,
        &anchor_recurrences,
        emitted.len(),
    );
    LoopCoverage {
        id: repeat.id.clone(),
        owner: repeat.owner.clone(),
        emitted_iterations: emitted.len(),
        matched_iterations: iterations.len(),
        matched_events: matched.len(),
        anchor_recurrences,
        signal_exits,
        body: Some(body),
        outside_body_candidate,
    }
}
