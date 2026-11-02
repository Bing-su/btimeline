use std::collections::BTreeMap;

use anyhow::{Context, Result, ensure};
use serde::Serialize;

use super::{
    Branch,
    Event,
    Loop,
    MAX_MS,
    Record,
    Recording,
    Timeline,
    replay_until,
    validate_repeat_support,
};

#[derive(Serialize)]
pub(crate) struct Upcoming {
    id: String,
    pub(crate) name: String,
    pub(crate) at_ms: i64,
    lead_ms: i64,
    duration_ms: i64,
    run: usize,
}

#[derive(Serialize)]
pub(crate) struct Forecast {
    at_ms: i64,
    until_ms: i64,
    pub(crate) status: &'static str,
    pub(crate) phase: Option<String>,
    run: usize,
    observations_used: usize,
    pub(crate) events: Vec<Upcoming>,
    pub(crate) diagnostics: Vec<Record>,
    pub(crate) projection_diagnostics: Vec<Record>,
    assumptions: [&'static str; 2],
    not_checked: [&'static str; 3],
}

// Resolve event identities from the authored tree, e.g. selected options and on-exit displays use the same event order key.
fn definitions<'a>(
    events: &'a [Event],
    branches: &'a [Branch],
    loops: &'a [Loop],
    out: &mut BTreeMap<usize, &'a Event>,
) {
    out.extend(events.iter().map(|event| (event.order, event)));
    for branch in branches {
        for option in &branch.options {
            definitions(&option.events, &[], &option.loops, out);
        }
        if let Some(merge) = &branch.merge {
            definitions(&merge.events, &[], &[], out);
        }
    }
    for repeat in loops {
        definitions(&repeat.events, &repeat.branches, &[], out);
        definitions(&repeat.on_exit, &[], &[], out);
    }
}

pub(crate) fn forecast(
    timeline: &Timeline,
    input: &Recording,
    at_ms: i64,
    horizon_ms: i64,
) -> Result<Forecast> {
    ensure!(
        (0..=input.end_ms).contains(&at_ms),
        "Forecast time must be inside the recording"
    );
    ensure!(horizon_ms > 0, "Forecast horizon must be positive");
    let until_ms = at_ms
        .checked_add(horizon_ms)
        .filter(|end| *end <= MAX_MS)
        .context("Forecast horizon exceeds the supported time range")?;
    validate_repeat_support(timeline, until_ms)?;
    let mut state = replay_until(timeline, input, at_ms)?;
    // Process every observation at the cutoff before looking ahead, e.g. a same-time wipe closes the phase first.
    state.settle(at_ms);
    let phase = state
        .phase
        .as_ref()
        .map(|phase| phase.definition.id.clone());
    let status = if state.fight.is_some() {
        "active"
    } else {
        "idle"
    };
    let run = state.run;
    let diagnostics = state
        .out
        .iter()
        .filter(|row| ["ambiguous", "violation", "unresolved"].contains(&row.action.as_str()))
        .cloned()
        .collect();
    let mut metadata = BTreeMap::new();
    definitions(&timeline.events, &[], &[], &mut metadata);
    for phase in &timeline.phases {
        definitions(&phase.events, &phase.branches, &phase.loops, &mut metadata);
    }
    let rows = state.finish(until_ms);
    let events = rows
        .iter()
        .filter(|row| row.action == "event" && row.at_ms > at_ms)
        .map(|row| {
            let event = row
                .event_order
                .and_then(|order| metadata.get(&order))
                .context("Missing forecast event definition")?;
            Ok((event.visible).then(|| Upcoming {
                id: event.id.clone(),
                name: event.name.clone(),
                at_ms: row.at_ms,
                lead_ms: row.at_ms - at_ms,
                duration_ms: event.duration * 100,
                run: row.run,
            }))
        })
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .flatten()
        .collect();
    let projection_diagnostics = rows
        .into_iter()
        .filter(|row| {
            row.at_ms > at_ms
                && ["ambiguous", "violation", "unresolved"].contains(&row.action.as_str())
        })
        .collect();
    Ok(Forecast {
        at_ms,
        until_ms,
        status,
        phase,
        run,
        observations_used: input
            .observations
            .iter()
            .take_while(|observation| observation.at_ms <= at_ms)
            .count(),
        events,
        diagnostics,
        projection_diagnostics,
        assumptions: [
            "No observations after at_ms; the current path continues under authored timers",
            "Unselected options and future signal-driven transitions, merges and exits are not predicted",
        ],
        not_checked: [
            "ACT signal equivalence",
            "game accuracy",
            "UI rendering and lookahead visibility",
        ],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selected_option_repeat_ignores_its_future_exit_and_merge() {
        let timeline = crate::timeline::parse(include_str!(
            "../../../tests/fixtures/timeline/option-loop.kdl"
        ))
        .expect("timeline");
        let recording = serde_json::from_str(include_str!(
            "../../../tests/fixtures/timeline/option-loop.json"
        ))
        .expect("recording");
        let result = forecast(&timeline, &recording, 1051, 3000).expect("forecast");
        assert_eq!(
            result
                .events
                .iter()
                .map(|event| (event.id.as_str(), event.at_ms, event.lead_ms))
                .collect::<Vec<_>>(),
            vec![("pulse", 2100, 1049), ("pulse", 3100, 2049)]
        );
        assert!(result.diagnostics.is_empty());
    }
}
