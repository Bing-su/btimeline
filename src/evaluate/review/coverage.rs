use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::{Value, json};

use super::Trace;
use crate::evaluate::coverage::{EvidenceRole, LoopCoverage, OptionCoverage};
use crate::timeline::Owner;

#[derive(Serialize)]
struct Choice {
    branch: String,
    option: String,
    owner: Owner,
    signal_selections: usize,
    #[serde(skip)]
    inputs: BTreeSet<String>,
    input_count: usize,
    first_observation: Option<Value>,
}
#[derive(Serialize)]
struct Repeat {
    id: String,
    owner: Owner,
    emitted_iterations: usize,
    matched_iterations: usize,
    matched_events: usize,
    consecutive_anchor_pairs: usize,
    signal_exits: usize,
    first_anchor_pair: Option<Value>,
    body_coverage_inputs: usize,
    complete_display_bodies: usize,
    incomplete_display_bodies: usize,
    unverified_iterations: usize,
    exits_with_matched_on_exit: usize,
    first_body_gap: Option<Value>,
}

impl Choice {
    fn observe(&mut self, input: &str, option: &OptionCoverage) -> Result<()> {
        self.signal_selections = self
            .signal_selections
            .checked_add(option.selections.len())
            .context("Coverage count overflow")?;
        if !option.selections.is_empty() {
            self.inputs.insert(input.into());
            self.input_count = self.inputs.len();
        }
        if self.first_observation.is_none() {
            self.first_observation = option
                .selections
                .first()
                .map(|selection| json!({"input":input,"selection":selection}));
        }
        Ok(())
    }
}
impl Repeat {
    fn observe(&mut self, input: &str, repeat: &LoopCoverage) -> Result<()> {
        for (sum, count) in [
            (&mut self.emitted_iterations, repeat.emitted_iterations),
            (&mut self.matched_iterations, repeat.matched_iterations),
            (&mut self.matched_events, repeat.matched_events),
            (
                &mut self.consecutive_anchor_pairs,
                repeat.anchor_recurrences.len(),
            ),
            (&mut self.signal_exits, repeat.signal_exits),
        ] {
            *sum = sum.checked_add(count).context("Coverage count overflow")?;
        }
        if self.first_anchor_pair.is_none() {
            self.first_anchor_pair = repeat
                .anchor_recurrences
                .first()
                .map(|pair| json!({"input":input,"pair":pair}));
        }
        if let Some(body) = &repeat.body {
            self.body_coverage_inputs += 1;
            for (sum, count) in [
                (
                    &mut self.complete_display_bodies,
                    body.complete_display_bodies,
                ),
                (
                    &mut self.incomplete_display_bodies,
                    body.incomplete_display_bodies,
                ),
                (&mut self.unverified_iterations, body.unverified_iterations),
                (
                    &mut self.exits_with_matched_on_exit,
                    body.exits_with_matched_on_exit,
                ),
            ] {
                *sum = sum
                    .checked_add(count)
                    .context("Body coverage count overflow")?;
            }
            if self.first_body_gap.is_none() {
                self.first_body_gap = body
                    .first_gap
                    .as_ref()
                    .map(|gap| json!({"input":input,"gap":gap}));
            }
        }
        Ok(())
    }
}

pub(super) fn summarize(traces: &[Trace], role: EvidenceRole, limit: usize) -> Result<Value> {
    let mut choices = BTreeMap::<(String, String), Choice>::new();
    let mut repeats = BTreeMap::<String, Repeat>::new();
    let mut available = 0;
    for trace in traces {
        let Some(coverage) = &trace.coverage else {
            continue;
        };
        available += 1;
        for branch in &coverage.branches {
            for option in &branch.options {
                let row = choices
                    .entry((branch.id.clone(), option.id.clone()))
                    .or_insert_with(|| Choice {
                        branch: branch.id.clone(),
                        option: option.id.clone(),
                        owner: branch.owner.clone(),
                        signal_selections: 0,
                        inputs: BTreeSet::new(),
                        input_count: 0,
                        first_observation: None,
                    });
                row.observe(&trace.input, option)?;
            }
        }
        for repeat in &coverage.loops {
            let row = repeats.entry(repeat.id.clone()).or_insert_with(|| Repeat {
                id: repeat.id.clone(),
                owner: repeat.owner.clone(),
                emitted_iterations: 0,
                matched_iterations: 0,
                matched_events: 0,
                consecutive_anchor_pairs: 0,
                signal_exits: 0,
                first_anchor_pair: None,
                body_coverage_inputs: 0,
                complete_display_bodies: 0,
                incomplete_display_bodies: 0,
                unverified_iterations: 0,
                exits_with_matched_on_exit: 0,
                first_body_gap: None,
            });
            row.observe(&trace.input, repeat)?;
        }
    }
    Ok(json!({
        "evidence_role": role, "available_inputs": available,
        "status": if available==0 {"unavailable"}else if traces.iter().filter(|t|t.status!="excluded").count()!=available {"partial"}else{"available"},
        "choices":choices.values().take(limit).collect::<Vec<_>>(),"omitted_choices":choices.len().saturating_sub(limit),
        "loops":repeats.values().take(limit).collect::<Vec<_>>(),"omitted_loops":repeats.len().saturating_sub(limit),
        "not_checked":["Unmodeled mechanics and unobserved loop boundaries", "Tests outside this manifest", "Independence of reports or declared roles"]
    }))
}
