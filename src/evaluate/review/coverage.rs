use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::{Value, json};

use super::{Target, Trace};
use crate::evaluate::coverage::{BranchCoverage, EvidenceRole, LoopCoverage, OptionCoverage};
use crate::timeline::Owner;

#[derive(Serialize)]
struct Choice {
    branch: String,
    option: String,
    signal_kind: Option<&'static str>,
    owner: Owner,
    signal_selections: usize,
    resumed_selections: usize,
    #[serde(skip)]
    inputs: BTreeSet<String>,
    input_count: usize,
    #[serde(skip)]
    reports: BTreeSet<String>,
    report_count: usize,
    fight_duration_range_ms: Option<[i64; 2]>,
    pace: Pace,
    first_forecast: ForecastCounts,
    first_observation: Option<Value>,
}
#[derive(Default, Serialize)]
struct Unresolved {
    count: usize,
    input_count: usize,
    first_observation: Option<Value>,
}
#[derive(Default, Serialize)]
struct Pace {
    cutoff_ms: Option<i64>,
    fast_inputs: usize,
    slow_inputs: usize,
}
#[derive(Default, Serialize)]
struct ForecastCounts {
    passed: usize,
    failed: usize,
    unverified: usize,
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
    fn observe(
        &mut self,
        trace: &Trace,
        branch: &BranchCoverage,
        option: &OptionCoverage,
    ) -> Result<()> {
        let input = &trace.input;
        self.signal_selections = self
            .signal_selections
            .checked_add(option.selections.len())
            .context("Coverage count overflow")?;
        self.resumed_selections += option
            .selections
            .iter()
            .filter(|selection| selection.resume_at_ms.is_some())
            .count();
        if !option.selections.is_empty() && self.inputs.insert(input.clone()) {
            self.input_count = self.inputs.len();
            if let Some((report, _)) = trace.trace.as_deref().and_then(|id| id.split_once('/')) {
                self.reports.insert(report.into());
                self.report_count = self.reports.len();
            }
            if let Some(duration) = trace.fight_duration_ms {
                let [min, max] = self.fight_duration_range_ms.unwrap_or([duration; 2]);
                self.fight_duration_range_ms = Some([min.min(duration), max.max(duration)]);
                self.pace.fast_inputs +=
                    usize::from(self.pace.cutoff_ms.is_some_and(|cutoff| duration <= cutoff));
                self.pace.slow_inputs +=
                    usize::from(self.pace.cutoff_ms.is_some_and(|cutoff| duration > cutoff));
            }
        }
        for selection in &option.selections {
            // A later selection in the same phase instance closes this sample.
            let end = branch
                .options
                .iter()
                .flat_map(|option| &option.selections)
                .filter(|other| {
                    other.run == selection.run
                        && other.phase_instance == selection.phase_instance
                        && other.at_ms > selection.at_ms
                })
                .map(|other| other.at_ms)
                .min();
            let forecast = trace
                .bosses
                .iter()
                .filter(|boss| {
                    Some(boss.phase.as_str()) == branch.owner.phase.as_deref()
                        && boss.entry_ms.is_some_and(|start| start <= selection.at_ms)
                        && boss.end_ms.is_some_and(|end| end >= selection.at_ms)
                })
                .flat_map(|boss| &boss.targets)
                .filter(|target| {
                    target.at_ms >= selection.at_ms
                        && end.is_none_or(|end| target.at_ms < end)
                        && belongs_to_option(target, option)
                })
                .min_by_key(|target| (target.at_ms, target.raw_index))
                .and_then(|target| target.forecast.as_ref());
            match forecast.map(|forecast| forecast.passed && forecast.diagnostics.is_empty()) {
                Some(true) => self.first_forecast.passed += 1,
                Some(false) => self.first_forecast.failed += 1,
                None => self.first_forecast.unverified += 1,
            }
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

fn belongs_to_option(target: &Target, option: &OptionCoverage) -> bool {
    // Shared bindings must respect the actual display when one was matched.
    target
        .matched_event
        .as_ref()
        .and_then(|matched| matched.pointer("/event/id"))
        .and_then(Value::as_str)
        .map_or_else(
            || {
                target
                    .event_ids
                    .iter()
                    .any(|id| option.event_ids.contains(id))
            },
            |id| option.event_ids.contains(id),
        )
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
    let mut durations = BTreeMap::<&str, Vec<i64>>::new();
    for trace in traces {
        if let (Some(coverage), Some(duration)) = (&trace.coverage, trace.fight_duration_ms) {
            for branch in &coverage.branches {
                durations.entry(&branch.id).or_default().push(duration);
            }
        }
    }
    let cutoffs = durations
        .into_iter()
        .filter_map(|(branch, mut values)| {
            values.sort_unstable();
            let median = *values.get(values.len().saturating_sub(1) / 2)?;
            let max = *values.last()?;
            // Split ties as a group, even when the median equals the maximum.
            let below_max = *values.iter().rev().find(|&&value| value < max)?;
            Some((branch, median.min(below_max)))
        })
        .collect::<BTreeMap<_, _>>();
    let mut choices = BTreeMap::<(String, String), Choice>::new();
    let mut unresolved = BTreeMap::<&str, Unresolved>::new();
    let mut repeats = BTreeMap::<String, Repeat>::new();
    let mut available = 0;
    for trace in traces {
        let Some(coverage) = &trace.coverage else {
            continue;
        };
        available += 1;
        for branch in &coverage.branches {
            if let Some(first) = branch.unresolved.first() {
                let row = unresolved.entry(&branch.id).or_default();
                row.count += branch.unresolved.len();
                row.input_count += 1;
                row.first_observation
                    .get_or_insert_with(|| json!({"input":trace.input,"observation":first}));
            }
            for option in &branch.options {
                let signal_kind = match option.fallback {
                    Some(true) => Some("fallback"),
                    Some(false) => Some("primary"),
                    None => None,
                };
                let row = choices
                    .entry((branch.id.clone(), option.id.clone()))
                    .or_insert_with(|| Choice {
                        branch: branch.id.clone(),
                        option: option.id.clone(),
                        signal_kind,
                        owner: branch.owner.clone(),
                        signal_selections: 0,
                        resumed_selections: 0,
                        inputs: BTreeSet::new(),
                        input_count: 0,
                        reports: BTreeSet::new(),
                        report_count: 0,
                        fight_duration_range_ms: None,
                        pace: Pace {
                            cutoff_ms: cutoffs.get(branch.id.as_str()).copied(),
                            ..Pace::default()
                        },
                        first_forecast: ForecastCounts::default(),
                        first_observation: None,
                    });
                row.observe(trace, branch, option)?;
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
        "unresolved_branches":unresolved.iter().take(limit).map(|(branch,row)| json!({"branch":branch,"count":row.count,"input_count":row.input_count,"first_observation":row.first_observation})).collect::<Vec<_>>(),
        "omitted_unresolved_branches":unresolved.len().saturating_sub(limit),
        "loops":repeats.values().take(limit).collect::<Vec<_>>(),"omitted_loops":repeats.len().saturating_sub(limit),
        "not_checked":["Unmodeled mechanics and unobserved loop boundaries", "Tests outside this manifest", "Independence of reports or declared roles"]
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choices_show_missing_pace_samples_and_first_owned_forecast() {
        let trace = |input, duration, selected, passed| {
            let option = |id| {
                json!({
                    "id":id,"event_ids":[id],
                    "selections": if id == selected {
                        json!([{"raw_index":2,"at_ms":20,"run":1,"phase_instance":1}])
                    } else { json!([]) }
                })
            };
            serde_json::from_value::<Trace>(json!({
                "input":input,"trace":format!("{input}/1"),"status":"evaluated",
                "fight_duration_ms":duration,
                "coverage":{"branches":[{"id":"fork","owner":{"phase":"boss","on_exit":false},
                    "options":[option("left"),option("right"),option("unseen")]}],"loops":[]},
                "bosses":[{"phase":"boss","entry_ms":0,"end_ms":100,"targets":[
                    {"raw_index":3,"at_ms":30,"id":"A","event_ids":["common",selected],
                     "matched_event":{"event":{"id":"common"}},"passed":false},
                    {"raw_index":4,"at_ms":40,"id":"B","event_ids":[selected],
                     "passed":passed,"forecast":{"passed":passed}}
                ]}]
            }))
            .unwrap()
        };
        let result = summarize(
            &[
                trace("one", 100, "left", true),
                trace("two", 200, "right", false),
            ],
            EvidenceRole::HeldOut,
            8,
        )
        .unwrap();
        let left = &result["choices"][0];
        let right = &result["choices"][1];
        assert_eq!(left["fight_duration_range_ms"], json!([100, 100]));
        assert_eq!(
            left["pace"],
            json!({"cutoff_ms":100,"fast_inputs":1,"slow_inputs":0})
        );
        assert_eq!(left["report_count"], 1);
        assert_eq!(left["first_forecast"]["passed"], 1);
        assert_eq!(right["pace"]["slow_inputs"], 1);
        assert_eq!(right["first_forecast"]["failed"], 1);
        assert_eq!(result["choices"][2]["input_count"], 0);
    }
}
